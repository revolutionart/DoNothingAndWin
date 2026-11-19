//! Preview builds Epic has published. The release feed lists only the newest
//! build of a branch, so the launcher remembers each preview it has seen. An
//! installed preview then keeps its name and stays recognisable as a preview
//! after Epic publishes the next preview or the final release.
use crate::{
    model::{self, Release},
    storage,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

/// Build identity to preview name. The name is empty when Epic supplied none.
type Known = BTreeMap<String, String>;

fn path() -> PathBuf {
    storage::data_dir().join("preview-builds.json")
}

fn known() -> &'static Mutex<Known> {
    static KNOWN: OnceLock<Mutex<Known>> = OnceLock::new();
    KNOWN.get_or_init(|| {
        // Tests must not depend on the previews recorded on the developer's machine.
        let known = if cfg!(test) {
            Known::default()
        } else {
            storage::read_json(&path()).unwrap_or_default()
        };
        Mutex::new(known)
    })
}

/// The version and branch a build belongs to, and its changelist.
fn identity(build: &str) -> Option<(String, u64)> {
    let (version_build, branch) = build.split_once("+++")?;
    let (version, changelist) = version_build.split_once('-')?;
    let changelist = changelist.parse().ok().filter(|value| *value > 0)?;
    Some((format!("{version}+++{branch}"), changelist))
}

fn remember(known: &mut Known, releases: &[Release]) -> bool {
    let mut changed = false;
    for release in releases.iter().filter(|release| release.preview) {
        let Some(build) = &release.build else {
            continue;
        };
        let label = release.preview_label.clone().unwrap_or_default();
        // Keep a recorded name when a later, partial feed supplies none.
        if known
            .get(build)
            .is_none_or(|old| *old != label && !label.is_empty())
        {
            known.insert(build.clone(), label);
            changed = true;
        }
    }
    changed
}

fn label_in(known: &Known, build: &str) -> Option<String> {
    known
        .get(build)
        .filter(|label| !label.is_empty())
        .cloned()
        .or_else(|| model::numbered_preview_label(build).map(str::to_owned))
}

fn is_preview_in(known: &Known, build: &str) -> bool {
    if known.contains_key(build) || model::numbered_preview_label(build).is_some() {
        return true;
    }
    // Epic publishes the final release after every preview of that version.
    // A build older than a known preview of the same version is an earlier preview,
    // including one the Epic Games Launcher installed while this launcher was closed.
    let Some((version, changelist)) = identity(build) else {
        return false;
    };
    known
        .keys()
        .map(String::as_str)
        .chain(model::NUMBERED_PREVIEWS.iter().map(|(build, _)| *build))
        .filter_map(identity)
        .any(|(other, newer)| other == version && changelist < newer)
}

/// Remember the preview builds in a release check. Call after each successful check.
pub fn record(releases: &[Release]) -> Result<(), String> {
    let mut known = known().lock().unwrap();
    if remember(&mut known, releases) {
        storage::write_json(&path(), &*known)?;
    }
    Ok(())
}

/// The name Epic gave an exact preview build, such as "Preview 3".
pub fn label(build: &str) -> Option<String> {
    label_in(&known().lock().unwrap(), build)
}

pub fn is_preview(build: &str) -> bool {
    is_preview_in(&known().lock().unwrap(), build)
        || crate::epic_registration::is_cached_preview(build)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(changelist: u32, label: Option<&str>) -> Release {
        Release {
            app: "UE_5.10".into(),
            catalog_id: String::new(),
            channel: "Live".into(),
            platform: "Windows".into(),
            build: Some(format!("5.10.0-{changelist}+++UE5+Release-5.10-Windows")),
            version: model::EngineVersion::parse("5.10.0"),
            preview: true,
            preview_label: label.map(str::to_owned),
        }
    }

    #[test]
    fn later_previews_keep_their_names_after_epic_replaces_them() {
        let mut known = Known::default();
        let third = preview(300, Some("Preview 3"));
        let fourth = preview(400, Some("Preview 4"));
        assert!(remember(&mut known, std::slice::from_ref(&third)));
        assert!(remember(&mut known, std::slice::from_ref(&fourth)));
        assert!(!remember(&mut known, std::slice::from_ref(&fourth)));
        // A partial feed without a name must not erase the recorded one.
        assert!(!remember(&mut known, &[preview(400, None)]));
        let third = third.build.unwrap();
        assert_eq!(label_in(&known, &third).as_deref(), Some("Preview 3"));
        assert_eq!(
            label_in(&known, fourth.build.as_deref().unwrap()).as_deref(),
            Some("Preview 4")
        );
        assert!(is_preview_in(&known, &third));
    }

    #[test]
    fn unseen_older_builds_are_previews_and_the_final_release_is_not() {
        let mut known = Known::default();
        remember(&mut known, &[preview(400, Some("Preview 4"))]);
        // Installed by the Epic Games Launcher while this launcher never saw it.
        let unseen = "5.10.0-350+++UE5+Release-5.10-Windows";
        assert!(is_preview_in(&known, unseen));
        assert_eq!(label_in(&known, unseen), None);
        assert!(!is_preview_in(
            &known,
            "5.10.0-500+++UE5+Release-5.10-Windows"
        ));
        assert!(!is_preview_in(
            &known,
            "5.10.1-350+++UE5+Release-5.10-Windows"
        ));
        assert!(!is_preview_in(
            &known,
            "5.8.4-350+++UE5+Release-5.8-Windows"
        ));
        assert!(!is_preview_in(&known, "not a build"));
        // The first 5.9 previews predate the recorded names.
        let empty = Known::default();
        assert!(is_preview_in(
            &empty,
            "5.9.0-60351404+++UE5+Release-5.9-Windows"
        ));
        assert!(is_preview_in(
            &empty,
            "5.9.0-60000000+++UE5+Release-5.9-Windows"
        ));
        assert!(!is_preview_in(
            &empty,
            "5.9.0-61000000+++UE5+Release-5.9-Windows"
        ));
    }

    #[test]
    fn stable_releases_are_never_recorded() {
        let mut known = Known::default();
        let mut stable = preview(500, None);
        stable.preview = false;
        assert!(!remember(&mut known, &[stable]));
        assert!(known.is_empty());
    }
}
