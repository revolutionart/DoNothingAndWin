//! Fab library acquisition dates, with legacy Epic entitlement dates as a fallback.
use super::Item;
use crate::epic::{Epic, Session};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub fn refresh(session: &Session, items: &mut [Item]) -> Result<(), String> {
    let epic = Epic::new()?;
    let library = epic.fab_library_session(session)?;
    let dates = fab_dates(|cursor| library.page(cursor))?;
    apply_fab(items, &dates);
    if items.iter().any(|item| item.acquired_at.is_none()) {
        // Legacy records are optional; failure must not discard Fab's dates.
        let _ = refresh_legacy(&epic, session, items);
    }
    Ok(())
}

fn fab_dates(
    mut page: impl FnMut(&str) -> Result<Value, String>,
) -> Result<HashMap<String, i64>, String> {
    let mut dates = HashMap::new();
    let mut cursor = String::new();
    let mut seen = HashSet::new();
    for _ in 0..1000 {
        let value = page(&cursor)?;
        let records = value["results"]
            .as_array()
            .ok_or("Invalid Fab acquisition response")?;
        for record in records {
            let (Some(id), Some(date)) = (
                record["listing"]["uid"].as_str(),
                timestamp(&record["createdAt"]),
            ) else {
                continue;
            };
            // The outer record is the acquisition; listing.createdAt is publication.
            dates
                .entry(id.to_owned())
                .and_modify(|d: &mut i64| *d = (*d).min(date))
                .or_insert(date);
        }
        let next = value
            .get("cursors")
            .and_then(|v| v.get("next"))
            .ok_or("Missing Fab acquisition cursor")?;
        cursor = match next {
            Value::Null => return Ok(dates),
            Value::String(next) if next.is_empty() => return Ok(dates),
            Value::String(next) => next.clone(),
            _ => return Err("Invalid Fab acquisition cursor".into()),
        };
        if records.is_empty() || !seen.insert(cursor.clone()) {
            return Err("Fab repeated an acquisition page".into());
        }
    }
    Err("Fab acquisition pagination limit reached".into())
}

fn timestamp(value: &Value) -> Option<i64> {
    let date = time::OffsetDateTime::parse(
        value.as_str()?,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()?
    .unix_timestamp();
    (date >= 0 && date <= crate::epic::now() as i64).then_some(date)
}

fn apply_fab(items: &mut [Item], dates: &HashMap<String, i64>) {
    for item in items {
        if let Some(date) = crate::fab_listing::listing_id(item).and_then(|id| dates.get(&id)) {
            item.acquired_at = Some(*date);
        }
    }
}

fn refresh_legacy(epic: &Epic, session: &Session, items: &mut [Item]) -> Result<(), String> {
    let mut grants = Vec::new();
    let mut seen = HashSet::new();
    for _ in 0..100 {
        let page = epic.entitlement_page(session, grants.len())?;
        let finished = page.len() < 1000;
        for grant in &page {
            let id = grant["id"].as_str().ok_or("Entitlement missing identity")?;
            if !seen.insert(id.to_owned()) {
                return Err("Repeated entitlement page".into());
            }
        }
        grants.extend(page);
        if finished {
            apply(items, &grants);
            return Ok(());
        }
    }
    Err("Entitlement pagination limit reached".into())
}

fn apply(items: &mut [Item], grants: &[Value]) {
    let mut dates: HashMap<(String, String), i64> = HashMap::new();
    for grant in grants {
        if grant["active"] != true || grant["status"] != "ACTIVE" {
            continue;
        }
        let (Some(namespace), Some(id), Some(date)) = (
            grant["namespace"].as_str(),
            grant["catalogItemId"].as_str(),
            grant["grantDate"].as_str(),
        ) else {
            continue;
        };
        let Ok(date) =
            time::OffsetDateTime::parse(date, &time::format_description::well_known::Rfc3339)
        else {
            continue;
        };
        let timestamp = date.unix_timestamp();
        if timestamp < 0 || timestamp > crate::epic::now() as i64 {
            continue;
        }
        // Multiple active grants for one item do not change the original acquisition date.
        dates
            .entry((namespace.into(), id.into()))
            .and_modify(|d| *d = (*d).min(timestamp))
            .or_insert(timestamp);
    }
    for item in items {
        if item.acquired_at.is_some() {
            continue;
        }
        item.acquired_at = std::iter::once(item.asset_id.as_str())
            .chain(item.legacy_item_id.as_deref())
            .filter_map(|id| {
                dates
                    .get(&(item.asset_namespace.clone(), id.to_owned()))
                    .copied()
            })
            .min();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn fab_dates_follow_cursors_and_use_first_acquisition_for_duplicate_licenses() {
        let mut calls = Vec::new();
        let dates = fab_dates(|cursor| {
            calls.push(cursor.to_owned());
            Ok(if cursor.is_empty() {
                json!({"results":[{"listing":{"uid":"same"},"createdAt":"2025-01-01T00:00:00Z"}],"cursors":{"next":"next-page"}})
            } else {
                json!({"results":[
                    {"listing":{"uid":"same"},"createdAt":"2024-01-01T00:00:00Z"},
                    {"listing":{"uid":"later-page"},"createdAt":"2023-01-01T00:00:00Z"},
                    {"listing":{"uid":"invalid"},"createdAt":"invalid"},
                    {"listing":{"uid":"future"},"createdAt":"2999-01-01T00:00:00Z"}
                ],"cursors":{"next":null}})
            })
        }).unwrap();
        assert_eq!(calls, ["", "next-page"]);
        assert_eq!(dates.len(), 2);
        assert_eq!(dates["same"], 1704067200);
        assert!(dates.contains_key("later-page"));
    }

    #[test]
    fn incomplete_or_repeating_acquisition_pages_are_errors() {
        let repeated = json!({"results":[{"listing":{"uid":"one"},"createdAt":"2024-01-01T00:00:00Z"}],"cursors":{"next":"same"}});
        assert!(fab_dates(|_| Ok(repeated.clone())).is_err());
        assert!(fab_dates(|_| Ok(json!({"results":[]}))).is_err());
        assert!(fab_dates(|_| Err("offline".into())).is_err());
    }

    #[test]
    fn unmatched_records_preserve_cached_dates_and_legacy_cannot_replace_fab_dates() {
        let mut items: Vec<Item> = serde_json::from_value(json!([
            {"assetId":"same","assetNamespace":"ue","title":"Cached","acquiredAt":42},
            {"assetId":"other","assetNamespace":"ue","title":"Unknown"}
        ]))
        .unwrap();
        apply_fab(
            &mut items,
            &HashMap::from([("unrelated-listing".into(), 123)]),
        );
        apply(
            &mut items,
            &[
                json!({"active":true,"status":"ACTIVE","namespace":"ue","catalogItemId":"same","grantDate":"2020-01-01T00:00:00Z"}),
            ],
        );
        assert_eq!(items[0].acquired_at, Some(42));
        assert_eq!(items[1].acquired_at, None);
    }
    #[test]
    fn fab_acquisition_records_put_newly_acquired_listing_first() {
        let mut items: Vec<Item> = serde_json::from_value(json!([
            {"assetId":"a","assetNamespace":"fab","title":"Alphabetical first","url":"https://www.fab.com/listings/11111111-1111-4111-8111-111111111111"},
            {"assetId":"b","assetNamespace":"fab","title":"Modular Post Apocalyptic Desert Environment","url":"https://www.fab.com/listings/22222222-2222-4222-8222-222222222222"}
        ])).unwrap();
        let records = vec![
            json!({"listing":{"uid":"11111111-1111-4111-8111-111111111111", "createdAt":"2025-09-01T00:00:00Z"},"createdAt":"2024-01-01T00:00:00Z"}),
            json!({"listing":{"uid":"22222222-2222-4222-8222-222222222222", "createdAt":"2020-01-01T00:00:00Z"},"createdAt":"2025-01-01T00:00:00.390321+00:00"}),
        ];
        let dates = fab_dates(|_| Ok(json!({"results":records,"cursors":{"next":null}}))).unwrap();
        apply_fab(&mut items, &dates);
        crate::fab_listing::sort(&mut items, crate::fab_listing::Sort::LatestAdded);
        assert_eq!(items[0].asset_id, "b");
        assert_eq!(items[0].acquired_at, Some(1735689600));
    }
    #[test]
    fn acquisition_requires_exact_identity_and_active_valid_grants() {
        let mut items: Vec<Item> = serde_json::from_value(json!([
            {"assetId":"new-id","assetNamespace":"ue","legacyItemId":"old-id","title":"Migrated"},
            {"assetId":"old-id","assetNamespace":"other","title":"Other namespace"},
            {"assetId":"missing","assetNamespace":"ue","title":"Unknown"}
        ]))
        .unwrap();
        let grant = json!({"active":true,"status":"ACTIVE","namespace":"ue","catalogItemId":"old-id","grantDate":"2020-01-01T00:00:00Z"});
        let mut earlier = grant.clone();
        earlier["grantDate"] = json!("2019-01-01T00:00:00Z");
        let mut revoked = grant.clone();
        revoked["active"] = json!(false);
        revoked["grantDate"] = json!("2010-01-01T00:00:00Z");
        let mut invalid = grant.clone();
        invalid["grantDate"] = json!("not a date");
        apply(&mut items, &[grant, earlier, revoked, invalid]);
        assert_eq!(items[0].acquired_at, Some(1546300800));
        assert_eq!(items[1].acquired_at, None);
        assert_eq!(items[2].acquired_at, None);
        let saved = serde_json::to_string(&items).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<Item>>(&saved).unwrap()[0].acquired_at,
            items[0].acquired_at
        );
    }
}
