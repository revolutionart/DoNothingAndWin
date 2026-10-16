use super::{Checked, Plugin};
use crate::{buildpatch, model::Installation};
use std::{
    collections::HashMap,
    fs,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const TTL: Duration = Duration::from_secs(300);
type Key = (String, String, String, std::path::PathBuf);
struct Entry {
    at: Instant,
    fingerprint: [u8; 20],
    row: Checked,
}
static CACHE: OnceLock<Mutex<HashMap<Key, Entry>>> = OnceLock::new();
fn key(account: &str, engine: &Installation, plugin: &Plugin) -> Key {
    (
        account.into(),
        engine.path.clone(),
        engine.version.clone(),
        plugin.descriptor.clone(),
    )
}
pub(super) fn fingerprint(plugin: &Plugin) -> Option<[u8; 20]> {
    if fs::metadata(&plugin.descriptor).ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let mut bytes = fs::read(&plugin.descriptor).ok()?;
    if let Some(root) = plugin.descriptor.parent() {
        let receipt = root.join(".dnw-plugin-install.json");
        if receipt.exists() {
            if fs::metadata(&receipt).ok()?.len() > 64 * 1024 {
                return None;
            }
            bytes.push(0);
            bytes.extend(fs::read(receipt).ok()?);
        }
    }
    Some(buildpatch::digest(&bytes))
}
fn reusable(entry: &Entry, fingerprint: [u8; 20], force: bool, now: Instant) -> bool {
    !force && entry.fingerprint == fingerprint && now.saturating_duration_since(entry.at) < TTL
}
pub(super) fn get(
    account: &str,
    engine: &Installation,
    plugin: &Plugin,
    force: bool,
) -> Option<Checked> {
    if force {
        return None;
    }
    let fingerprint = fingerprint(plugin)?;
    let cache = CACHE.get_or_init(Default::default).lock().unwrap();
    let entry = cache.get(&key(account, engine, plugin))?;
    reusable(entry, fingerprint, force, Instant::now()).then(|| entry.row.clone())
}
pub(super) fn remember(
    account: &str,
    engine: &Installation,
    row: &Checked,
    fingerprint: Option<[u8; 20]>,
) {
    // Network/authentication failures must never turn into a fresh successful result.
    let Some(fingerprint) = fingerprint.filter(|_| row.available_version.is_some()) else {
        return;
    };
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    cache.retain(|_, e| e.at.elapsed() < TTL);
    if cache.len() >= 128 {
        cache.clear();
    }
    cache.insert(
        key(account, engine, &row.plugin),
        Entry {
            at: Instant::now(),
            fingerprint,
            row: row.clone(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Plugin, Checked) {
        let dir = tempfile::tempdir().unwrap();
        let descriptor = dir.path().join("Test.uplugin");
        fs::write(&descriptor, br#"{"VersionName":"1.0"}"#).unwrap();
        let plugin = Plugin {
            name: "Test".into(),
            version: "1.0".into(),
            number: 1,
            catalog_id: None,
            descriptor,
        };
        let row = Checked {
            plugin: plugin.clone(),
            update: None,
            available_version: Some("1.0".into()),
            status: "Up to date".into(),
        };
        (dir, plugin, row)
    }
    #[test]
    fn cache_respects_expiry_force_and_descriptor_changes() {
        let (_dir, plugin, row) = fixture();
        let hash = fingerprint(&plugin).unwrap();
        let now = Instant::now();
        let entry = Entry {
            at: now,
            fingerprint: hash,
            row,
        };
        assert!(reusable(&entry, hash, false, now));
        assert!(!reusable(&entry, hash, true, now));
        assert!(!reusable(&entry, hash, false, now + TTL));
        fs::write(&plugin.descriptor, br#"{"VersionName":"0.9"}"#).unwrap();
        assert!(!reusable(&entry, fingerprint(&plugin).unwrap(), false, now));
    }
    #[test]
    fn results_are_isolated_by_account_engine_and_receipt() {
        let (dir, plugin, row) = fixture();
        let engine = Installation {
            path: "engine-one".into(),
            version: "5.6.1".into(),
            association: "5.6".into(),
        };
        remember("account-a", &engine, &row, fingerprint(&plugin));
        assert!(get("account-a", &engine, &plugin, false).is_some());
        assert!(get("account-b", &engine, &plugin, false).is_none());
        let other = Installation {
            path: "engine-two".into(),
            ..engine.clone()
        };
        assert!(get("account-a", &other, &plugin, false).is_none());
        fs::write(
            dir.path().join(".dnw-plugin-install.json"),
            b"changed receipt",
        )
        .unwrap();
        assert!(get("account-a", &engine, &plugin, false).is_none());
    }
}
