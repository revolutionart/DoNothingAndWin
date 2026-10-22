//! Fab's public curated promotions. Never claims products or changes entitlements.
use crate::{fab_listing, storage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, io::Read, time::Duration};

pub const PAGE_URL: &str = "https://www.fab.com/limited-time-free";
const FEED_URL: &str = "https://www.fab.com/i/blades/free_content_blade";
const CACHE_SECONDS: i64 = 6 * 60 * 60;

#[derive(Clone, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub title: String,
    pub publisher: String,
    pub thumbnail: Option<String>,
    pub ends_at: Option<i64>,
}
impl Offer {
    pub fn url(&self) -> String {
        format!("https://www.fab.com/listings/{}", self.id)
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Catalogue {
    pub checked_at: i64,
    pub offers: Vec<Offer>,
}
impl Catalogue {
    pub fn ends_at(&self) -> Option<i64> {
        self.offers.iter().filter_map(|o| o.ends_at).min()
    }
    pub fn needs_refresh(&self, now: i64) -> bool {
        now < self.checked_at
            || now.saturating_sub(self.checked_at)
                >= if self.offers.is_empty() {
                    900
                } else {
                    CACHE_SECONDS
                }
            || self.ends_at().is_some_and(|end| now >= end)
    }
}
fn image_url(raw: &str) -> bool {
    reqwest::Url::parse(raw).is_ok_and(|url| {
        url.scheme() == "https"
            && matches!(url.host_str(), Some("media.fab.com" | "cdn1.epicgames.com"))
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
    })
}
pub fn load_cache() -> Option<Catalogue> {
    let path = storage::data_dir().join("fab-free-offers.json");
    if std::fs::metadata(&path).ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let catalogue: Catalogue = storage::read_json(&path)?;
    (catalogue.offers.len() <= 30
        && catalogue.offers.iter().all(|offer| {
            fab_listing::listing_url_id(&offer.url()).as_deref() == Some(&offer.id)
                && offer.ends_at.is_some()
                && offer.thumbnail.as_ref().is_none_or(|url| image_url(url))
        }))
    .then_some(catalogue)
}
pub fn fetch() -> Result<Catalogue, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("DoNothingAndWin/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "Could not start the Fab offer check")?;
    let response = client
        .get(FEED_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .map_err(|_| "Fab did not respond. Try again later.")?;
    if !response.status().is_success() {
        return Err(format!(
            "Fab returned HTTP {}. View the offers on Fab or try again later.",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read Fab offers")?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Fab offer response exceeded the size limit".into());
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Fab returned an unreadable offer list")?;
    let catalogue = parse(&value, time::OffsetDateTime::now_utc().unix_timestamp())?;
    storage::write_json(
        &storage::data_dir().join("fab-free-offers.json"),
        &catalogue,
    )?;
    Ok(catalogue)
}
fn timestamp(value: &Value) -> Option<i64> {
    time::OffsetDateTime::parse(
        value.as_str()?,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()
    .map(|t| t.unix_timestamp())
}
fn parse(value: &Value, now: i64) -> Result<Catalogue, String> {
    if value["isLimitedFreeContent"] != true || value["blockContentType"] != "listings" {
        return Err(
            "Fab's limited-time offer feed has changed. Open Fab for the current offers.".into(),
        );
    }
    let tiles = value["tiles"]
        .as_array()
        .ok_or("Fab did not provide an offer list")?;
    if tiles.len() > 30 {
        return Err("Fab returned an unexpectedly large offer list".into());
    }
    let mut seen = HashSet::new();
    let mut offers = Vec::new();
    for tile in tiles {
        let listing = &tile["listing"];
        let id = listing["uid"]
            .as_str()
            .ok_or("Offer listing identity is missing")?;
        let id = fab_listing::listing_url_id(&format!("https://www.fab.com/listings/{id}"))
            .ok_or("Invalid offer listing identity")?;
        if !seen.insert(id.clone()) {
            return Err("Fab repeated an offer listing".into());
        }
        let title = listing["title"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or("Offer title is missing")?;
        let licenses = listing["licenses"]
            .as_array()
            .filter(|l| !l.is_empty())
            .ok_or("Offer pricing is unavailable")?;
        let mut ends = Vec::new();
        let mut eligible = true;
        for license in licenses {
            let price = &license["priceTier"];
            let discounted = price["discountedPrice"].as_f64();
            let discount = price["discountSettings"]["discountPercentage"].as_f64();
            if discounted != Some(0.)
                || discount != Some(100.)
                || price["price"].as_f64().is_none_or(|p| p <= 0.)
            {
                eligible = false;
                break;
            }
            let start =
                timestamp(&price["discountStartDate"]).ok_or("Offer start time is unavailable")?;
            let end =
                timestamp(&price["discountEndDate"]).ok_or("Offer end time is unavailable")?;
            if end <= start {
                return Err("Fab returned an invalid offer period".into());
            }
            if now < start || now >= end {
                eligible = false;
                break;
            }
            ends.push(end);
        }
        if !eligible {
            continue;
        }
        let thumbnail = listing["thumbnails"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|t| t["images"].as_array().into_iter().flatten())
            .filter_map(|image| Some((image["width"].as_i64()?, image["url"].as_str()?)))
            .filter(|(_, url)| image_url(url))
            .min_by_key(|(width, _)| (width - 640).abs())
            .map(|(_, url)| url.to_owned());
        offers.push(Offer {
            id,
            title: title.into(),
            publisher: listing["user"]["sellerName"]
                .as_str()
                .unwrap_or("Publisher unavailable")
                .into(),
            thumbnail,
            ends_at: ends.into_iter().min(),
        });
    }
    Ok(Catalogue {
        checked_at: now,
        offers,
    })
}
pub fn countdown(end: i64, now: i64) -> String {
    let seconds = end.saturating_sub(now);
    if seconds <= 0 {
        return "Offer ended".into();
    }
    if seconds < 60 {
        return "Ends in less than a minute".into();
    }
    format!(
        "Ends in {}d {}h {}m",
        seconds / 86400,
        seconds % 86400 / 3600,
        seconds % 3600 / 60
    )
}
pub fn end_label(end: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(end)
        .map(|t| {
            format!(
                "Next offer ends {} {}, {} at {:02}:{:02} UTC",
                t.month(),
                t.day(),
                t.year(),
                t.hour(),
                t.minute()
            )
        })
        .unwrap_or_else(|_| "See Fab for offer end time".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        serde_json::json!({"blockContentType":"listings","isLimitedFreeContent":true,"tiles":[{"listing":{
            "uid":"be5ed72e-f542-41f4-b747-bdb343eef609","title":"Example promotion","user":{"sellerName":"Publisher"},
            "licenses":[{"priceTier":{"price":10.,"discountedPrice":0.,"discountSettings":{"discountPercentage":100.},"discountStartDate":"2026-10-20T14:00:00Z","discountEndDate":"2026-11-03T14:59:00Z"}}],
            "thumbnails":[{"images":[{"width":320,"url":"https://media.fab.com/small.jpg"},{"width":640,"url":"https://media.fab.com/card.jpg"}]}]
        }}]})
    }
    #[test]
    fn uses_verified_promotion_window_and_thumbnail_not_heading_or_publication_date() {
        let v = fixture();
        let start =
            timestamp(&v["tiles"][0]["listing"]["licenses"][0]["priceTier"]["discountStartDate"])
                .unwrap();
        let c = parse(&v, start).unwrap();
        assert_eq!(c.offers.len(), 1);
        assert_eq!(
            c.offers[0].thumbnail.as_deref(),
            Some("https://media.fab.com/card.jpg")
        );
        let end = c.ends_at().unwrap();
        assert_eq!(
            end_label(end),
            "Next offer ends November 3, 2026 at 14:59 UTC"
        );
        assert!(parse(&v, start - 1).unwrap().offers.is_empty());
        assert_eq!(parse(&v, end - 1).unwrap().offers.len(), 1);
        assert!(parse(&v, end).unwrap().offers.is_empty());
    }
    #[test]
    fn excludes_permanently_free_and_partial_discounts_and_fails_on_missing_dates() {
        for (field, number) in [("price", 0.), ("discountedPrice", 1.)] {
            let mut v = fixture();
            v["tiles"][0]["listing"]["licenses"][0]["priceTier"][field] = number.into();
            assert!(parse(&v, 1793000000).unwrap().offers.is_empty());
        }
        let mut v = fixture();
        v["tiles"][0]["listing"]["licenses"][0]["priceTier"]["discountEndDate"] = Value::Null;
        assert!(parse(&v, 1793000000).is_err());
        assert!(parse(&serde_json::json!({"tiles":[]}), 1793000000).is_err());
        let mut v = fixture();
        v["tiles"][0]["listing"]["uid"] = "../wrong".into();
        assert!(parse(&v, 1793000000).is_err());
    }
    #[test]
    fn expiry_and_stale_cache_never_keep_a_live_countdown() {
        let now = 1793000000;
        let mut c = parse(&fixture(), now).unwrap();
        assert!(!c.needs_refresh(now));
        assert!(c.needs_refresh(now - 1));
        assert!(c.needs_refresh(now + CACHE_SECONDS));
        c.checked_at = c.ends_at().unwrap() - 10;
        assert!(c.needs_refresh(c.ends_at().unwrap()));
        assert_eq!(countdown(now, now), "Offer ended");
        assert_eq!(countdown(now + 90061, now), "Ends in 1d 1h 1m");
    }
    #[test]
    #[ignore = "Read-only live public Fab offer feed check; no account needed"]
    fn live_public_offers() {
        let c = fetch().unwrap();
        println!(
            "{} current offers; end {:?}",
            c.offers.len(),
            c.ends_at().map(end_label)
        );
        for o in c.offers {
            println!("{} — {}", o.title, o.url());
            if let Some(url) = o.thumbnail {
                assert!(crate::fab::thumbnail_image(&url).is_ok());
            }
        }
    }
}
