use serde::Deserialize;
use std::{io::Read, time::Duration};

const REPOSITORY: &str = "https://github.com/revolutionart/DoNothingAndWin";
const API: &str = "https://api.github.com/repos/revolutionart/DoNothingAndWin/releases/latest";
const MAX_RESPONSE: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub version: String,
    pub release_url: String,
    pub download_url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    published_at: Option<String>,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    state: String,
    size: u64,
    browser_download_url: String,
}

pub fn check(current: &str, portable: bool) -> Result<Option<Update>, String> {
    // Public metadata only. Never attach the Epic session or any GitHub token.
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("DoNothingAndWin/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not start the launcher update check.".to_string())?;
    let response = client
        .get(API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|_| "Could not reach GitHub. Check your connection and try again.".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "GitHub update check unavailable (HTTP {}). Try again later.",
            response.status().as_u16()
        ));
    }
    let mut data = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut data)
        .map_err(|_| "Could not read the launcher release details.".to_string())?;
    if data.len() as u64 > MAX_RESPONSE {
        return Err("Launcher release response is too large.".into());
    }
    parse(&data, current, portable)
}

fn parse(data: &[u8], current: &str, portable: bool) -> Result<Option<Update>, String> {
    let release: Release = serde_json::from_slice(data)
        .map_err(|_| "Invalid launcher release details.".to_string())?;
    if release.draft
        || release.prerelease
        || release.published_at.as_deref().is_none_or(str::is_empty)
    {
        return Err("No published stable launcher release is available.".into());
    }
    let current = semver::Version::parse(current)
        .map_err(|_| "Invalid installed launcher version.".to_string())?;
    let version = semver::Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|_| "Unrecognized launcher release version.".to_string())?;
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err("No stable launcher release is available.".into());
    }
    if (version.major, version.minor, version.patch)
        <= (current.major, current.minor, current.patch)
    {
        return Ok(None);
    }
    let name = if portable {
        format!("DoNothingAndWin-Portable-{version}.zip")
    } else {
        format!("DoNothingAndWin-Setup-{version}.exe")
    };
    let download_url = format!("{REPOSITORY}/releases/download/{}/{name}", release.tag_name);
    if !release.assets.iter().any(|asset| {
        asset.name == name
            && asset.state == "uploaded"
            && asset.size > 0
            && asset.browser_download_url == download_url
    }) {
        return Err(
            "The new release does not yet have a download for this launcher edition.".into(),
        );
    }
    Ok(Some(Update {
        version: version.to_string(),
        release_url: format!("{REPOSITORY}/releases/tag/{}", release.tag_name),
        download_url,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the public GitHub release API"]
    fn live_public_release_has_both_download_variants() {
        for portable in [false, true] {
            let update = check("0.0.0", portable)
                .expect("public release check")
                .expect("published release");
            assert!(update.release_url.starts_with(REPOSITORY));
            assert!(
                update
                    .download_url
                    .ends_with(if portable { ".zip" } else { ".exe" })
            );
        }
    }
    fn release(version: &str) -> serde_json::Value {
        serde_json::json!({"tag_name":format!("v{version}"),"draft":false,"prerelease":false,"published_at":"2026-10-15T12:00:00Z","assets":[
            {"name":format!("DoNothingAndWin-Setup-{version}.exe"),"state":"uploaded","size":1,"browser_download_url":format!("{REPOSITORY}/releases/download/v{version}/DoNothingAndWin-Setup-{version}.exe")},
            {"name":format!("DoNothingAndWin-Portable-{version}.zip"),"state":"uploaded","size":1,"browser_download_url":format!("{REPOSITORY}/releases/download/v{version}/DoNothingAndWin-Portable-{version}.zip")}
        ]})
    }
    fn parse_value(
        value: &serde_json::Value,
        current: &str,
        portable: bool,
    ) -> Result<Option<Update>, String> {
        parse(&serde_json::to_vec(value).unwrap(), current, portable)
    }
    #[test]
    fn numeric_comparison_and_correct_download_variant() {
        let release = release("1.10.0");
        let installed = parse_value(&release, "1.9.0", false).unwrap().unwrap();
        let portable = parse_value(&release, "1.9.0", true).unwrap().unwrap();
        assert!(installed.download_url.ends_with("Setup-1.10.0.exe"));
        assert!(portable.download_url.ends_with("Portable-1.10.0.zip"));
        assert_eq!(parse_value(&release, "1.10.0", false).unwrap(), None);
        assert_eq!(parse_value(&release, "2.0.0", false).unwrap(), None);
    }
    #[test]
    fn drafts_previews_incomplete_assets_and_foreign_urls_are_rejected() {
        for field in ["draft", "prerelease"] {
            let mut r = release("1.2.0");
            r[field] = true.into();
            assert!(parse_value(&r, "1.0.0", false).is_err());
        }
        assert!(parse_value(&release("1.2.0-rc.1"), "1.0.0", false).is_err());
        let mut r = release("1.2.0");
        r["published_at"] = serde_json::Value::Null;
        assert!(parse_value(&r, "1.0.0", false).is_err());
        let mut r = release("1.2.0");
        r["assets"][0]["browser_download_url"] = "https://example.com/app.exe".into();
        assert!(parse_value(&r, "1.0.0", false).is_err());
        assert!(parse_value(&r, "1.0.0", true).unwrap().is_some());
        let mut r = release("1.2.0");
        r["assets"] = serde_json::json!([]);
        assert!(parse_value(&r, "1.0.0", false).is_err());
        assert!(parse(b"not json", "1.0.0", false).is_err());
    }
}
