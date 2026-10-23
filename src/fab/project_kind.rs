//! Package type evidence from verified manifest file lists; no content is downloaded.
use super::{Item, Version};
use serde::{Deserialize, Serialize};

const RECHECK_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone, Serialize, Deserialize)]
pub struct Detection {
    fingerprint: String,
    contains_project: bool,
    checked_at: u64,
}

fn versions(item: &Item) -> Vec<&Version> {
    item.project_versions
        .iter()
        .filter(|v| {
            v.target_platforms.is_empty() || v.target_platforms.iter().any(|p| p == "Windows")
        })
        .collect()
}

fn fingerprint(item: &Item) -> String {
    let mut builds: Vec<_> = versions(item)
        .into_iter()
        .map(|v| {
            let mut listed: Vec<_> = v
                .build_versions
                .iter()
                .filter(|b| b.platform == "Windows")
                .map(|b| b.build_version.as_str())
                .collect();
            listed.sort_unstable();
            (v.artifact_id.as_str(), listed)
        })
        .collect();
    builds.sort_unstable();
    super::cache_key(&serde_json::to_string(&(item.key(), builds)).unwrap())
}

pub fn signature(item: &Item) -> String {
    fingerprint(item)
}

pub fn known(item: &Item) -> bool {
    item.distribution_method != "ASSET_PACK"
        || item
            .project_detection
            .as_ref()
            .is_some_and(|d| d.fingerprint == fingerprint(item))
}

pub fn is_project(item: &Item) -> bool {
    item.project_detection
        .as_ref()
        .is_some_and(|d| d.contains_project && d.fingerprint == fingerprint(item))
}

pub fn needs_refresh(item: &Item) -> bool {
    item.distribution_method == "ASSET_PACK"
        && (!known(item)
            || item.project_detection.as_ref().is_some_and(|d| {
                crate::epic::now().saturating_sub(d.checked_at) >= RECHECK_SECONDS
            }))
}

fn cache_path(item: &Item) -> std::path::PathBuf {
    crate::storage::data_dir()
        .join("library/package-types")
        .join(format!("{}.json", fingerprint(item)))
}

pub fn hydrate(items: &mut [Item]) {
    for item in items
        .iter_mut()
        .filter(|i| i.distribution_method == "ASSET_PACK")
    {
        if !known(item) {
            item.project_detection = None;
        }
        if let Some(detection) = crate::storage::read_json::<Detection>(&cache_path(item)) {
            apply(item, detection);
        }
    }
}

pub fn apply(item: &mut Item, detection: Detection) -> bool {
    if item.distribution_method != "ASSET_PACK" || detection.fingerprint != fingerprint(item) {
        return false;
    }
    if item.project_detection.as_ref().is_some_and(|old| {
        old.fingerprint == detection.fingerprint && old.checked_at > detection.checked_at
    }) {
        return false;
    }
    item.project_detection = Some(detection);
    true
}

pub fn detect(session: &crate::epic::Session, item: &Item) -> Result<Detection, String> {
    let detection = detect_with(item, |version| {
        super::package(session, item, version).map(|(manifest, _)| manifest)
    })?;
    let _ = crate::storage::write_json(&cache_path(item), &detection);
    Ok(detection)
}

fn detect_with(
    item: &Item,
    mut manifest: impl FnMut(&Version) -> Result<crate::buildpatch::Manifest, String>,
) -> Result<Detection, String> {
    let mut checked = std::collections::HashSet::new();
    let mut first_error = None;
    // Newest artifacts first; stop as soon as a project descriptor is proven.
    for version in versions(item).into_iter().rev() {
        if !checked.insert(&version.artifact_id) {
            continue;
        }
        match manifest(version) {
            Ok(manifest)
                if manifest
                    .files
                    .iter()
                    .any(|f| f.name.to_ascii_lowercase().ends_with(".uproject")) =>
            {
                return Ok(Detection {
                    fingerprint: fingerprint(item),
                    contains_project: true,
                    checked_at: crate::epic::now(),
                });
            }
            Ok(_) => {}
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    // A failed/absent manifest is unknown, never evidence of an asset-only package.
    if let Some(error) = first_error {
        return Err(error);
    }
    if checked.is_empty() {
        return Err("No Windows package manifest is available.".into());
    }
    Ok(Detection {
        fingerprint: fingerprint(item),
        contains_project: false,
        checked_at: crate::epic::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Read-only live manifest classification; requires a connected account and PROJECT_KIND_TEST_TITLE"]
    fn live_owned_asset_pack_project_detection() {
        let title = std::env::var("PROJECT_KIND_TEST_TITLE").expect("Set exact owned item title");
        let session = crate::storage::load_session().expect("Connect Epic first");
        let mut item = super::super::load_library_cache(&session.account_id)
            .into_iter()
            .find(|item| item.title == title)
            .expect("Owned item");
        assert_eq!(item.distribution_method, "ASSET_PACK");
        item.project_detection = None;
        assert!(!item.complete_project(), "No title/ID exceptions");
        let detection = detect(&session, &item).expect("Read verified manifest file lists");
        apply(&mut item, detection);
        assert!(item.complete_project());
        // Reuse the actual detection result for native UI rendering.
        crate::storage::write_json(
            std::path::Path::new("target/project-kind-verified-item.json"),
            &item,
        )
        .unwrap();
        println!(
            "Verified {} as a complete project from manifest evidence; no package content downloaded.",
            item.title
        );
    }
    fn item() -> Item {
        serde_json::from_value(serde_json::json!({
            "assetId":"any-project", "assetNamespace":"any-publisher",
            "title":"Any sample", "distributionMethod":"ASSET_PACK",
            "projectVersions":[{"artifactId":"sample58","engineVersions":["5.8"]}]
        }))
        .unwrap()
    }
    #[test]
    fn arbitrary_asset_pack_with_verified_project_evidence_uses_create_project() {
        let mut item = item();
        item.project_detection = Some(Detection {
            fingerprint: fingerprint(&item),
            contains_project: true,
            checked_at: crate::epic::now(),
        });
        assert!(
            item.complete_project(),
            "Manifest evidence must override ASSET_PACK for every publisher"
        );
    }

    fn manifest(names: &[&str]) -> crate::buildpatch::Manifest {
        crate::buildpatch::Manifest {
            files: names
                .iter()
                .map(|name| crate::buildpatch::File {
                    name: (*name).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn verified_nested_descriptor_changes_type_and_survives_cache_round_trip() {
        let mut item = item();
        let detection = detect_with(&item, |_| {
            Ok(manifest(&[
                "Sample/EXAMPLE.UPROJECT",
                "Sample/Content/Map.umap",
            ]))
        })
        .unwrap();
        let raw = serde_json::to_vec(&detection).unwrap();
        assert!(apply(&mut item, serde_json::from_slice(&raw).unwrap()));
        assert!(item.complete_project());
        assert!(known(&item));
        assert!(!needs_refresh(&item));
        item.title = "Renamed example".into();
        assert!(item.complete_project());
    }

    #[test]
    fn content_only_packages_remain_assets() {
        let mut item = item();
        let detection = detect_with(&item, |_| {
            Ok(manifest(&["Content/Map.umap", "readme.uproject.txt"]))
        })
        .unwrap();
        apply(&mut item, detection);
        assert!(known(&item));
        assert!(!item.complete_project());
    }

    #[test]
    fn failed_manifest_is_unknown_not_an_asset_pack_verdict() {
        let mut item = item();
        item.project_versions.push(
            serde_json::from_value(serde_json::json!({
                "artifactId":"other","engineVersions":["5.7"]
            }))
            .unwrap(),
        );
        assert!(
            detect_with(&item, |v| if v.artifact_id == "other" {
                Err("Network failure".into())
            } else {
                Ok(manifest(&["Content/Map.umap"]))
            })
            .is_err()
        );
        assert!(!known(&item));
        assert!(needs_refresh(&item));
        // A proven descriptor in another artifact still settles the question.
        let detection = detect_with(&item, |v| {
            if v.artifact_id == "other" {
                Err("Network failure".into())
            } else {
                Ok(manifest(&["Sample.uproject"]))
            }
        })
        .unwrap();
        apply(&mut item, detection);
        assert!(item.complete_project());
    }

    #[test]
    fn checks_all_unique_windows_artifacts_until_a_descriptor_is_found() {
        let mut item = item();
        item.project_versions.push(item.project_versions[0].clone());
        item.project_versions.push(
            serde_json::from_value(serde_json::json!({
                "artifactId":"mac-only","targetPlatforms":["Mac"],"engineVersions":["5.8"]
            }))
            .unwrap(),
        );
        item.project_versions.push(
            serde_json::from_value(serde_json::json!({
                "artifactId":"sample-new","engineVersions":["5.9"]
            }))
            .unwrap(),
        );
        let mut seen = Vec::new();
        let detection = detect_with(&item, |v| {
            seen.push(v.artifact_id.clone());
            Ok(if v.artifact_id == "sample58" {
                manifest(&["Sample.uproject"])
            } else {
                manifest(&["Content/Map.umap"])
            })
        })
        .unwrap();
        assert_eq!(seen, ["sample-new", "sample58"]);
        apply(&mut item, detection);
        assert!(item.complete_project());
    }

    #[test]
    fn changed_artifacts_or_builds_invalidate_evidence_and_stale_worker_results() {
        let mut item = item();
        let detection = detect_with(&item, |_| Ok(manifest(&["Sample.uproject"]))).unwrap();
        apply(&mut item, detection.clone());
        item.project_versions[0]
            .build_versions
            .push(super::super::LibraryBuild {
                platform: "Windows".into(),
                build_version: "new-build".into(),
            });
        assert!(!known(&item));
        assert!(!item.complete_project());
        assert!(!apply(&mut item, detection));
        assert!(needs_refresh(&item));
    }

    #[test]
    fn old_evidence_remains_useful_while_a_background_recheck_runs() {
        let mut item = item();
        let mut detection = detect_with(&item, |_| Ok(manifest(&["Sample.uproject"]))).unwrap();
        detection.checked_at = 0;
        apply(&mut item, detection);
        assert!(known(&item));
        assert!(item.complete_project());
        assert!(needs_refresh(&item));
    }

    #[test]
    fn library_sync_cannot_replace_new_evidence_with_an_older_snapshot() {
        let mut item = item();
        let mut old = detect_with(&item, |_| Ok(manifest(&["Content/Map.umap"]))).unwrap();
        old.checked_at = 1;
        let current = detect_with(&item, |_| Ok(manifest(&["Sample.uproject"]))).unwrap();
        apply(&mut item, current);
        assert!(!apply(&mut item, old));
        assert!(item.complete_project());
    }

    #[test]
    fn missing_manifests_and_other_package_types_are_not_reclassified() {
        let mut item = item();
        item.project_versions.clear();
        assert!(detect_with(&item, |_| panic!("No manifest to fetch")).is_err());
        item = self::item();
        let evidence = detect_with(&item, |_| Ok(manifest(&["Sample.uproject"]))).unwrap();
        item.distribution_method = "CODE_PLUGIN".into();
        assert!(!apply(&mut item, evidence));
        assert!(!item.complete_project());
        assert!(!needs_refresh(&item));
    }
}
