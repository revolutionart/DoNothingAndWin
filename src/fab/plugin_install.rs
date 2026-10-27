//! Resolve and install a plugin independently for each selected engine.
use super::{Item, Version};
use crate::{buildpatch::Manifest, epic::Session, installer, model::Installation};
use std::{collections::HashSet, path::Path};

pub(super) struct Candidate {
    version: Version,
    pub(super) manifest: Manifest,
    pub(super) bases: Vec<reqwest::Url>,
    name: String,
    number: i64,
}

pub(super) fn descriptor_file(manifest: &Manifest) -> Result<&crate::buildpatch::File, String> {
    let descriptors: Vec<_> = manifest
        .files
        .iter()
        .filter(|f| f.name.ends_with(".uplugin"))
        .collect();
    if descriptors.len() != 1 || descriptors[0].size() > 2 * 1024 * 1024 {
        return Err("Package must contain one bounded plugin descriptor".into());
    }
    installer::relative(&descriptors[0].name)?;
    Ok(descriptors[0])
}

fn same_branch(a: &str, b: &str) -> bool {
    crate::model::EngineVersion::parse(a)
        .zip(crate::model::EngineVersion::parse(b))
        .is_some_and(|(a, b)| a.branch() == b.branch())
}

pub(super) fn resolve_with(
    item: &Item,
    engine: &str,
    wait_for_library: bool,
    mut fetch: impl FnMut(&Version) -> Result<(Manifest, Vec<reqwest::Url>, serde_json::Value), String>,
) -> Result<Candidate, String> {
    if !item.plugin() {
        return Err("Only plugins can be installed into engines".into());
    }
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for version in item
        .project_versions
        .iter()
        .filter(|v| v.supports_windows_engine(engine))
    {
        if !seen.insert(&version.artifact_id) {
            continue;
        }
        // An unreadable candidate could be newer. Never silently fall back to an older one.
        let (manifest, bases, descriptor) = fetch(version)?;
        descriptor_file(&manifest)?;
        if manifest.app != version.artifact_id || !same_branch(&manifest.build, engine) {
            return Err(
                "Downloaded manifest does not match the selected artifact and engine".into(),
            );
        }
        if !same_branch(descriptor["EngineVersion"].as_str().unwrap_or(""), engine) {
            return Err("Plugin descriptor does not confirm compatibility with this engine".into());
        }
        if wait_for_library {
            version.require_listed_windows_build(&manifest.build)?;
        }
        candidates.push(Candidate {
            version: version.clone(),
            manifest,
            bases,
            name: descriptor["VersionName"]
                .as_str()
                .unwrap_or("Unknown")
                .into(),
            number: descriptor["Version"]
                .as_i64()
                .ok_or("Missing plugin version number")?,
        });
    }
    if candidates.is_empty() {
        return Err("No compatible Windows build for this engine".into());
    }
    // Compare verified publisher versions, never artifact suffixes or response order.
    let mut best = 0;
    for index in 1..candidates.len() {
        let a = &candidates[index];
        let b = &candidates[best];
        if crate::plugins::compare_versions(a.number, &a.name, b.number, &b.name)
            == Some(std::cmp::Ordering::Greater)
        {
            best = index;
        }
    }
    let chosen = &candidates[best];
    for (index, other) in candidates.iter().enumerate() {
        if index == best {
            continue;
        }
        if crate::plugins::compare_versions(chosen.number, &chosen.name, other.number, &other.name)
            != Some(std::cmp::Ordering::Greater)
        {
            return Err(
                "Multiple compatible packages cannot be ordered safely; no plugin was installed"
                    .into(),
            );
        }
    }
    Ok(candidates.swap_remove(best))
}

fn run_targets(
    engines: &[Installation],
    control: &installer::Control,
    mut install: impl FnMut(usize, &Installation) -> Result<String, String>,
    mut finished: impl FnMut(&Installation, &Result<String, String>),
) -> Result<String, String> {
    if engines.is_empty() {
        return Err("Choose at least one engine".into());
    }
    let mut succeeded = 0;
    let mut failures = Vec::new();
    for (index, engine) in engines.iter().enumerate() {
        if control.check().is_err() {
            break;
        }
        let result = super::transfer_result(install(index, engine), control);
        if let Err(error) = &result {
            failures.push(format!("UE {}: {error}", engine.version));
        } else {
            succeeded += 1;
        }
        finished(engine, &result);
    }
    if control.check().is_err() && succeeded < engines.len() {
        return Err(format!(
            "Stopped. {succeeded} of {} engines completed; remaining installations were cancelled. Completed installations are unchanged.",
            engines.len()
        ));
    }
    if failures.is_empty() {
        Ok(format!("Plugin installed for {succeeded} engine(s)."))
    } else {
        Err(format!(
            "Installed for {succeeded} of {} engines. {}",
            engines.len(),
            failures.join("; ")
        ))
    }
}

pub fn install_many(
    session: &Session,
    selected: &Item,
    engines: &[Installation],
    control: &installer::Control,
    wait_for_library: bool,
    progress: impl Fn(String),
    finished: impl FnMut(&Installation, &Result<String, String>),
) -> Result<String, String> {
    control.check()?;
    crate::download_policy::check_install_allowed()?;
    progress("Refreshing available plugin builds…".into());
    let library_result = super::library(session, |_| {});
    control.check()?;
    let library = library_result?;
    let item = library
        .iter()
        .find(|item| item.key() == selected.key() && item.plugin())
        .ok_or("This plugin is no longer available in the connected Fab library")?;
    let mut paths = HashSet::new();
    run_targets(
        engines,
        control,
        |index, engine| {
            let target = Path::new(&engine.path);
            installer::no_links(target)?;
            let canonical = std::fs::canonicalize(target).map_err(|e| e.to_string())?;
            if !paths.insert(canonical.clone()) {
                return Err("Engine selected more than once".into());
            }
            let actual = crate::platform::inspect_engine(&canonical, &engine.association)
                .ok_or("Selected engine is no longer installed")?;
            if actual.version != engine.version {
                return Err("Engine version changed; reopen the install dialog".into());
            }
            crate::maintenance::ensure_closed(&canonical)?;
            let label = format!(
                "UE {} • Engine {} of {}",
                engine.version,
                index + 1,
                engines.len()
            );
            progress(format!("{label} • Checking the latest compatible plugin…"));
            let candidate = resolve_with(item, &engine.version, wait_for_library, |version| {
                control.check()?;
                let (manifest, bases) = super::package(session, item, version)?;
                control.check()?;
                let file = descriptor_file(&manifest)?;
                let raw =
                    crate::plugins::payload(&manifest, &bases, file, &mut Default::default())?;
                let descriptor =
                    serde_json::from_slice(&raw).map_err(|_| "Invalid plugin descriptor")?;
                Ok((manifest, bases, descriptor))
            })?;
            let name = crate::plugins::display_version(&candidate.name);
            super::install_package(
                item,
                &candidate.version,
                &canonical,
                &engine.version,
                || Ok((candidate.manifest, candidate.bases)),
                control,
                |status| {
                    progress(format!(
                        "{} • Version {name} • {label} • {status}",
                        item.title
                    ))
                },
            )?;
            Ok(format!(
                "Installed {} (version {name}) for Unreal Engine {}",
                item.title, engine.version
            ))
        },
        finished,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[ignore = "Read-only manifest and descriptor audit; requires connected Epic and PLUGIN_PACKAGE_AUDIT_TITLE"]
    fn owned_windows_package_size_audit() {
        let title = std::env::var("PLUGIN_PACKAGE_AUDIT_TITLE").unwrap();
        let branch = std::env::var("PLUGIN_PACKAGE_AUDIT_ENGINE").unwrap();
        let session = crate::storage::load_session().expect("Connect Epic first");
        let item = crate::fab::load_library_cache(&session.account_id)
            .into_iter()
            .find(|i| i.title == title && i.plugin())
            .expect("Owned plugin");
        let candidate = resolve_with(&item, &branch, true, |version| {
            let (manifest, bases) = crate::fab::package(&session, &item, version)?;
            let file = descriptor_file(&manifest)?;
            let raw = crate::plugins::payload(&manifest, &bases, file, &mut Default::default())?;
            let descriptor =
                serde_json::from_slice(&raw).map_err(|e| format!("Descriptor: {e}"))?;
            Ok((manifest, bases, descriptor))
        })
        .unwrap();
        let manifest = candidate.manifest;
        let descriptor =
            crate::installer::relative(&descriptor_file(&manifest).unwrap().name).unwrap();
        let prefix = descriptor.parent().unwrap();
        let mut all = Vec::new();
        let mut windows = Vec::new();
        for file in &manifest.files {
            let path = crate::installer::relative(&file.name).unwrap();
            if let Ok(relative) = path.strip_prefix(prefix) {
                all.push(file);
                if crate::plugins::package_files::windows_file(relative) {
                    windows.push(file);
                }
            }
        }
        let size = |files: &[&crate::buildpatch::File]| -> (u64, u64) {
            let mut ids = HashSet::new();
            let raw = files.iter().map(|f| f.size()).sum();
            let compressed = files
                .iter()
                .flat_map(|f| &f.parts)
                .filter(|p| ids.insert(p.guid.clone()))
                .map(|p| manifest.chunks[&p.guid].size)
                .sum();
            (raw, compressed)
        };
        println!(
            "{} publisher version {} | artifact {} | build {}",
            item.title, candidate.name, manifest.app, manifest.build
        );
        let (raw, compressed) = size(&all);
        println!(
            "Complete package: {} files | {} prepared bytes ({:.3} GB) | {} compressed chunk bytes ({:.3} GB / {:.3} GiB)",
            all.len(),
            raw,
            raw as f64 / 1e9,
            compressed,
            compressed as f64 / 1e9,
            compressed as f64 / 1073741824.
        );
        let (raw, compressed) = size(&windows);
        println!(
            "Windows selection: {} files | {} prepared bytes ({:.3} GB) | {} compressed chunk bytes ({:.3} GB / {:.3} GiB)",
            windows.len(),
            raw,
            raw as f64 / 1e9,
            compressed,
            compressed as f64 / 1e9,
            compressed as f64 / 1073741824.
        );
        crate::storage::write_json(Path::new("target/pcgex-current-manifest.json"), &manifest)
            .unwrap();
    }

    fn item() -> Item {
        serde_json::from_value(json!({"assetId":"pcgex","assetNamespace":"test","title":"PCGEx","distributionMethod":"CODE_PLUGIN","projectVersions":[
            {"artifactId":"pcg58","engineVersions":["UE_5.8"],"targetPlatforms":["Windows"]},
            {"artifactId":"pcg56new","engineVersions":["UE_5.6"],"targetPlatforms":["Windows"]},
            {"artifactId":"pcg56old","engineVersions":["UE_5.6"],"targetPlatforms":["Windows"]},
            {"artifactId":"mac56","engineVersions":["UE_5.6"],"targetPlatforms":["Mac"]}
        ]})).unwrap()
    }
    fn package(v: &Version) -> (Manifest, Vec<reqwest::Url>, serde_json::Value) {
        let (branch, name) = match v.artifact_id.as_str() {
            "pcg58" => ("5.8.0", "0.79.2"),
            "pcg56new" => ("5.6.0", "0.76.1"),
            "pcg56old" => ("5.6.0", "0.75.0"),
            _ => panic!("incompatible artifact fetched"),
        };
        (
            Manifest {
                app: v.artifact_id.clone(),
                build: format!("{branch}-123-Windows"),
                files: vec![crate::buildpatch::File {
                    name: "PCGEx/PCGEx.uplugin".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            vec![],
            json!({"EngineVersion":branch,"Version":1,"VersionName":name}),
        )
    }
    #[test]
    fn each_engine_gets_its_latest_compatible_version_independent_of_api_order() {
        let mut item = item();
        for _ in 0..2 {
            for (engine, expected) in [("5.6.1", "0.76.1"), ("5.8.3", "0.79.2")] {
                let candidate = resolve_with(&item, engine, false, |v| Ok(package(v))).unwrap();
                assert_eq!(candidate.name, expected);
            }
            assert!(resolve_with(&item, "5.7.4", false, |_| panic!("must not fetch")).is_err());
            item.project_versions.reverse();
        }
    }
    #[test]
    fn rejects_mismatched_artifact_build_descriptor_and_unlisted_package() {
        for fault in 0..4 {
            let result = resolve_with(&item(), "5.8.3", fault == 3, |v| {
                let (mut m, b, mut d) = package(v);
                match fault {
                    0 => m.app = "other".into(),
                    1 => m.build = "5.6.0-123".into(),
                    2 => d["EngineVersion"] = json!("5.6.0"),
                    _ => {}
                }
                Ok((m, b, d))
            });
            assert!(result.is_err());
        }
        let mut item = item();
        item.project_versions[0]
            .build_versions
            .push(super::super::LibraryBuild {
                platform: "Windows".into(),
                build_version: "5.8.0-123-Windows".into(),
            });
        assert!(resolve_with(&item, "5.8.3", true, |v| Ok(package(v))).is_ok());
    }
    #[test]
    fn failed_or_ambiguous_candidates_do_not_silently_install_an_older_build() {
        assert!(
            resolve_with(&item(), "5.6.1", false, |v| {
                if v.artifact_id == "pcg56new" {
                    Err("Unavailable".into())
                } else {
                    Ok(package(v))
                }
            })
            .is_err()
        );
        for label in ["0.76.1", "Preview"] {
            assert!(
                resolve_with(&item(), "5.6.1", false, |v| {
                    let (m, b, mut d) = package(v);
                    if v.artifact_id == "pcg56old" {
                        d["VersionName"] = json!(label);
                    }
                    Ok((m, b, d))
                })
                .is_err()
            );
        }
    }
    #[test]
    fn stopping_current_engine_preserves_completed_work_and_never_starts_remaining_engine() {
        let engines: Vec<_> = ["5.6.1", "5.7.4", "5.8.3"]
            .into_iter()
            .map(|v| Installation {
                version: v.into(),
                path: v.into(),
                association: v.into(),
            })
            .collect();
        let control = installer::Control::default();
        let mut started = vec![];
        let mut outcomes = vec![];
        let result = run_targets(
            &engines,
            &control,
            |index, _| {
                started.push(index);
                if index == 1 {
                    control.stop();
                    control.check()?;
                }
                Ok("Installed".into())
            },
            |engine, result| outcomes.push((engine.version.clone(), result.clone())),
        );
        assert_eq!(started, vec![0, 1]);
        assert!(outcomes[0].1.is_ok());
        assert!(outcomes[1].1.as_ref().unwrap_err().starts_with("Stopped."));
        assert!(
            result
                .unwrap_err()
                .starts_with("Stopped. 1 of 3 engines completed")
        );
        assert!(
            run_targets(
                &engines,
                &control,
                |_, _| panic!("must not start"),
                |_, _| panic!("no outcomes")
            )
            .is_err()
        );
    }
    #[test]
    fn batch_reports_each_engine_and_continues_after_a_target_fails() {
        let engines: Vec<_> = ["5.6.1", "5.7.4", "5.8.3"]
            .into_iter()
            .map(|v| Installation {
                version: v.into(),
                path: v.into(),
                association: v.into(),
            })
            .collect();
        let mut outcomes = vec![];
        let result = run_targets(
            &engines,
            &installer::Control::default(),
            |index, _| {
                if index == 1 {
                    Err("Engine in use".into())
                } else {
                    Ok("Installed".into())
                }
            },
            |e, r| outcomes.push((e.version.clone(), r.is_ok())),
        );
        assert_eq!(
            outcomes,
            vec![
                ("5.6.1".into(), true),
                ("5.7.4".into(), false),
                ("5.8.3".into(), true)
            ]
        );
        assert!(result.unwrap_err().contains("Installed for 2 of 3 engines"));
    }
}
