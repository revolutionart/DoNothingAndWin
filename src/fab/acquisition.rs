//! Account entitlement grant dates. Listing publication and local discovery are not acquisition dates.
use super::Item;
use crate::epic::{Epic, Session};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub fn refresh(session: &Session, items: &mut [Item]) -> Result<(), String> {
    let epic = Epic::new()?;
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
