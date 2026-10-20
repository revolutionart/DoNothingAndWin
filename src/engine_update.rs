//! Update an existing official engine without changing its path or installed components.
use crate::{
    buildpatch,
    epic::Session,
    installer::{self, Control, Plan, Progress},
    maintenance,
    model::{EngineVersion, Installation, Release, Snapshot},
    storage,
};
use std::{
    collections::HashSet,
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

pub fn run(
    engine: &Installation,
    release: &Release,
    session: &Session,
    snapshot: &Snapshot,
    control: &Control,
    progress: Arc<Mutex<Progress>>,
) -> Result<String, String> {
    crate::download_policy::check_install_allowed()?;
    validate_release(engine, release)?;
    installer::no_links(Path::new(&engine.path))?;
    let original = fs::canonicalize(&engine.path).map_err(|e| e.to_string())?;
    let _lock = maintenance::lock(&original)?;
    maintenance::ensure_closed(&original)?;
    control.check()?;
    progress.lock().unwrap().phase = "Loading installed components and update manifest".into();
    let old = maintenance::prepare(engine, Some(session), snapshot)?;
    let mut next = installer::prepare(session, release, original.parent().unwrap())?;
    next.target = original.clone();
    next.selected = old.selected.clone();
    let key = buildpatch::hex(&buildpatch::digest(
        format!("{}:{}", original.display(), next.manifest.build).as_bytes(),
    ));
    let work = original
        .parent()
        .unwrap()
        .join(format!(".dnw-update-{key}"));
    installer::no_links(&work)?;
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let ready = work.join("ready");
    let previous = work.join("previous");
    installer::no_links(&ready)?;
    installer::no_links(&previous)?;
    if previous.exists() {
        return Err(format!(
            "A previous update backup needs attention. Your previous engine is preserved at {}.",
            previous.display()
        ));
    }
    control.check()?;
    let mut staged = next.clone();
    staged.target = ready.clone();
    if !ready.exists() {
        installer::stage_update(&staged, &original, control, progress.clone())?;
    }
    // A cancelled attempt can leave a completed staging directory. Reuse it only
    // after checking its build, component selection and every selected file.
    verify_staged(&staged, control, &progress)?;
    preserve_custom(&original, &ready, &old, &next, control, &progress)?;
    control.check()?;
    maintenance::ensure_closed(&original)?;
    let current = crate::platform::inspect_engine(&original, "")
        .ok_or("The installed engine changed during preparation. Refresh engines and retry.")?;
    if current.version != engine.version {
        return Err(
            "The installed engine changed during preparation. Refresh engines and retry.".into(),
        );
    }
    // Do not interrupt these two renames: cancellation before commit leaves the
    // original untouched. A failed second rename restores the original folder.
    phase(&progress, "Applying verified engine update");
    commit(&original, &ready, &previous, |from, to| {
        fs::rename(from, to)
    })?;
    phase(
        &progress,
        "Registering updated engine with Epic Games Launcher",
    );
    let registration = crate::epic_registration::register(&next).err();
    let receipt = storage::write_json(
        &original.join(".alpine-epic-registration.json"),
        &serde_json::json!({"registered":registration.is_none(),"warning":registration}),
    )
    .err();
    // The backup is a directory we just moved here, inside this exact transaction.
    // Reused files are hard links, so removing it does not remove the new files.
    phase(&progress, "Removing previous engine backup");
    let cleanup = fs::remove_dir_all(&previous).err();
    phase(&progress, "Engine update complete");
    let mut message = format!(
        "Updated Unreal Engine to {}.",
        release.version.as_ref().unwrap()
    );
    if let Some(error) = registration.or(receipt) {
        message.push_str(&format!(" Epic registration needs attention: {error}"));
    }
    if let Some(error) = cleanup {
        message.push_str(&format!(
            " Old backup remains at {}: {error}",
            previous.display()
        ));
    }
    Ok(message)
}

fn validate_release(engine: &Installation, release: &Release) -> Result<(), String> {
    let installed =
        EngineVersion::parse(&engine.version).ok_or("Unknown installed engine version")?;
    let version = release
        .version
        .as_ref()
        .ok_or("Update version is unresolved")?;
    if release.platform != "Windows"
        || release.channel != "Live"
        || release.app != format!("UE_{}", installed.branch())
        || version.branch() != installed.branch()
        || version <= &installed
        || release
            .build
            .as_deref()
            .and_then(EngineVersion::parse)
            .as_ref()
            != Some(version)
    {
        return Err("This release is not a newer compatible engine hotfix.".into());
    }
    Ok(())
}

// Byte percentages belong to one measured phase, not the entire update.
fn phase(progress: &Mutex<Progress>, name: &str) {
    let mut p = progress.lock().unwrap();
    p.phase = name.into();
    p.detail.clear();
    p.completed = 0;
    p.total = 0;
    p.files = 0;
    p.files_total = 0;
}

fn verify_staged(plan: &Plan, control: &Control, progress: &Mutex<Progress>) -> Result<(), String> {
    phase(progress, "Final verification of updated engine files");
    let files = plan.files();
    {
        let mut p = progress.lock().unwrap();
        p.total = files.iter().map(|file| file.size()).sum();
        p.files_total = files.len();
    }
    let receipt: serde_json::Value =
        storage::read_json(&plan.target.join(".engine-workspace-install.json"))
            .ok_or("Update staging is incomplete. The installed engine has not been changed.")?;
    if receipt["build"] != plan.manifest.build
        || receipt["app"] != plan.manifest.app
        || receipt["selected_tags"] != serde_json::json!(plan.selected)
    {
        return Err("Staged update has different components or build. The installed engine has not been changed.".into());
    }
    for file in files {
        control.check()?;
        progress.lock().unwrap().detail = file.name.clone();
        let path = plan.target.join(installer::relative(&file.name)?);
        installer::no_links(&path)?;
        if !fs::metadata(&path).is_ok_and(|m| m.len() == file.size()) {
            return Err(format!("Staged update failed verification: {}", file.name));
        }
        let hash = installer::hash_file_progress(&path, &mut |bytes| {
            control.check()?;
            let mut p = progress.lock().unwrap();
            p.completed += bytes;
            p.read += bytes;
            Ok(())
        })?;
        if hash != file.sha {
            return Err(format!("Staged update failed verification: {}", file.name));
        }
        progress.lock().unwrap().files += 1;
    }
    Ok(())
}

fn preserve_custom(
    original: &Path,
    ready: &Path,
    old: &Plan,
    next: &Plan,
    control: &Control,
    progress: &Mutex<Progress>,
) -> Result<(), String> {
    // Only old manifest-owned files may disappear. Everything else, including
    // Marketplace plugins and their receipts, is retained at its existing path.
    let owned: HashSet<_> = old.files().iter().map(|f| f.name.to_lowercase()).collect();
    let wanted: HashSet<_> = next.files().iter().map(|f| f.name.to_lowercase()).collect();
    // Refresh extras on retry; don't resurrect files deleted since an earlier attempt.
    phase(progress, "Checking staged plugins and custom files");
    let mut staged_dirs = vec![ready.to_path_buf()];
    while let Some(directory) = staged_dirs.pop() {
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            control.check()?;
            let path = entry.map_err(|e| e.to_string())?.path();
            installer::no_links(&path)?;
            let name = path
                .strip_prefix(ready)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/")
                .to_lowercase();
            {
                let mut p = progress.lock().unwrap();
                p.files += 1;
                p.detail = format!("{} entries checked • {name}", p.files);
            }
            if path.is_dir() {
                staged_dirs.push(path);
            } else if !wanted.contains(&name)
                && !matches!(
                    name.as_str(),
                    ".engine-workspace-install.json" | ".engine-workspace-manifest.json.gz"
                )
            {
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
    }
    phase(progress, "Preserving installed plugins and custom files");
    let mut directories = vec![original.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            control.check()?;
            let path = entry.map_err(|e| e.to_string())?.path();
            installer::no_links(&path)?;
            let relative = path.strip_prefix(original).map_err(|e| e.to_string())?;
            let name = relative.to_string_lossy().replace('\\', "/").to_lowercase();
            {
                let mut p = progress.lock().unwrap();
                p.files += 1;
                p.detail = format!("{} entries checked • {}", p.files, relative.display());
            }
            if matches!(
                name.as_str(),
                ".engine-workspace-manifest.json.gz"
                    | ".engine-workspace-install.json"
                    | ".alpine-epic-registration.json"
                    | ".engine-workspace-repair"
            ) {
                continue;
            }
            let destination = ready.join(relative);
            installer::no_links(&destination)?;
            if path.is_dir() {
                fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
                directories.push(path);
            } else if !owned.contains(&name) {
                if wanted.contains(&name) {
                    return Err(format!(
                        "The update conflicts with a custom file: {}",
                        relative.display()
                    ));
                }
                if destination.exists() {
                    // Refresh a custom file retained by a prior cancelled attempt.
                    fs::remove_file(&destination).map_err(|e| e.to_string())?;
                }
                fs::hard_link(&path, &destination).map_err(|e| {
                    format!("Cannot preserve custom file {}: {e}", relative.display())
                })?;
            }
        }
    }
    Ok(())
}

fn commit(
    original: &Path,
    ready: &Path,
    previous: &Path,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), String> {
    rename(original, previous)
        .map_err(|e| format!("Cannot update engine folder; close applications using it: {e}"))?;
    if let Err(error) = rename(ready, original) {
        if let Err(restore) = rename(previous, original) {
            return Err(format!(
                "Update commit failed: {error}. Automatic restore failed: {restore}. Your original engine is preserved at {}.",
                previous.display()
            ));
        }
        return Err(format!(
            "Update commit failed; original engine restored: {error}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildpatch::{Chunk, File, Manifest, Part};

    fn plan(target: &Path, patch: u32) -> (Plan, Vec<u8>) {
        let version = format!(
            r#"{{"MajorVersion":5,"MinorVersion":8,"PatchVersion":{patch},"Changelist":123}}"#
        );
        let editor = b"fixture editor";
        let data = [editor.as_slice(), version.as_bytes()].concat();
        let guid = "00000001000000020000000300000004".to_string();
        let files = [
            ("Engine/Binaries/Win64/UnrealEditor.exe", 0, editor.len()),
            ("Engine/Build/Build.version", editor.len(), version.len()),
        ]
        .into_iter()
        .map(|(name, offset, size)| File {
            name: name.into(),
            sha: buildpatch::digest(&data[offset..offset + size]),
            parts: vec![Part {
                guid: guid.clone(),
                offset: offset as u32,
                size: size as u32,
            }],
            ..Default::default()
        })
        .collect();
        let chunk = Chunk {
            guid: guid.clone(),
            sha: buildpatch::digest(&data),
            window: data.len() as u32,
            size: data.len() as u64,
            ..Default::default()
        };
        (
            Plan {
                manifest: Manifest {
                    app: "UE_5.8".into(),
                    build: format!("5.8.{patch}-123+++UE5+Release-5.8-Windows"),
                    files,
                    chunks: [(guid, chunk)].into_iter().collect(),
                    ..Default::default()
                },
                target: target.into(),
                bases: vec![],
                tags: vec![],
                selected: vec![],
            },
            data,
        )
    }

    fn put(root: &Path, path: &str, bytes: &[u8]) {
        let dest = root.join(path);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(dest, bytes).unwrap();
    }

    #[test]
    fn staged_hotfix_reuses_files_preserves_plugins_and_commits_at_original_path() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("UE_5.8");
        let ready = temp.path().join("ready");
        let (mut old, old_data) = plan(&original, 3);
        for f in old.files() {
            let part = &f.parts[0];
            put(
                &original,
                &f.name,
                &old_data[part.offset as usize..][..part.size as usize],
            );
        }
        put(
            &original,
            "Engine/Plugins/Marketplace/Custom/plugin.dll",
            b"my plugin",
        );
        put(&original, "Engine/obsolete.dat", b"old manifest-owned file");
        old.manifest.files.push(File {
            name: "Engine/obsolete.dat".into(),
            ..Default::default()
        });
        let (staged, data) = plan(&ready, 4);
        let key = buildpatch::hex(&buildpatch::digest(
            format!("{}:{}", staged.manifest.app, staged.manifest.build).as_bytes(),
        ));
        let work = temp
            .path()
            .join(format!(".engine-workspace-{}", &key[..16]));
        put(&work, "chunks/00000001000000020000000300000004", &data);
        let progress = Arc::new(Mutex::new(Progress::default()));
        let stopped = Control::default();
        stopped.stop();
        assert!(installer::stage_update(&staged, &original, &stopped, progress.clone()).is_err());
        assert_eq!(
            crate::platform::inspect_engine(&original, "")
                .unwrap()
                .version,
            "5.8.3"
        );
        installer::stage_update(&staged, &original, &Control::default(), progress.clone()).unwrap();
        assert_eq!(
            progress.lock().unwrap().phase,
            "Update files prepared - final verification pending"
        );
        assert_eq!(progress.lock().unwrap().total, 0);
        verify_staged(&staged, &Control::default(), &progress).unwrap();
        {
            let p = progress.lock().unwrap();
            assert_eq!(p.phase, "Final verification of updated engine files");
            assert_eq!(p.completed, data.len() as u64);
            assert_eq!(p.total, data.len() as u64);
            assert_eq!(p.files, staged.files().len());
            assert_eq!(p.files_total, staged.files().len());
            assert!(!p.detail.is_empty());
        }
        assert_eq!(progress.lock().unwrap().network, 0);
        assert_eq!(
            progress.lock().unwrap().written,
            data.len() as u64 - b"fixture editor".len() as u64
        );
        assert_eq!(
            crate::platform::inspect_engine(&original, "")
                .unwrap()
                .version,
            "5.8.3"
        );
        preserve_custom(
            &original,
            &ready,
            &old,
            &staged,
            &Control::default(),
            &progress,
        )
        .unwrap();
        {
            let p = progress.lock().unwrap();
            assert_eq!(p.phase, "Preserving installed plugins and custom files");
            assert_eq!(p.total, 0, "Unmeasured work must not display 100%");
            assert!(p.files > 0);
            assert!(!p.detail.is_empty());
        }
        let backup = temp.path().join("previous");
        commit(&original, &ready, &backup, |a, b| fs::rename(a, b)).unwrap();
        assert_eq!(
            crate::platform::inspect_engine(&original, "")
                .unwrap()
                .version,
            "5.8.4"
        );
        assert_eq!(
            fs::read(original.join("Engine/Plugins/Marketplace/Custom/plugin.dll")).unwrap(),
            b"my plugin"
        );
        assert!(!original.join("Engine/obsolete.dat").exists());
        assert_eq!(
            crate::platform::inspect_engine(&backup, "")
                .unwrap()
                .version,
            "5.8.3"
        );
        fs::remove_dir_all(backup).unwrap();
        assert_eq!(
            fs::read(original.join("Engine/Binaries/Win64/UnrealEditor.exe")).unwrap(),
            b"fixture editor"
        );
        put(&original, "Engine/Build/Build.version", b"corrupt");
        let mut committed = staged;
        committed.target = original;
        assert!(verify_staged(&committed, &Control::default(), &progress).is_err());
        {
            let p = progress.lock().unwrap();
            assert_eq!(p.phase, "Final verification of updated engine files");
            assert!(p.files < p.files_total);
            assert!(p.completed < p.total);
            assert_eq!(p.detail, "Engine/Build/Build.version");
        }
        assert!(
            verify_staged(&committed, &stopped, &progress)
                .unwrap_err()
                .starts_with("Stopped.")
        );
        assert_eq!(progress.lock().unwrap().completed, 0);
    }

    #[test]
    fn failed_swap_restores_original_and_keeps_verified_update() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("engine");
        let ready = temp.path().join("ready");
        let previous = temp.path().join("previous");
        put(&original, "marker", b"old");
        put(&ready, "marker", b"new");
        let result = commit(&original, &ready, &previous, |a, b| {
            if a == ready {
                Err(std::io::Error::other("injected access denied"))
            } else {
                fs::rename(a, b)
            }
        });
        assert!(result.unwrap_err().contains("original engine restored"));
        assert_eq!(fs::read(original.join("marker")).unwrap(), b"old");
        assert_eq!(fs::read(ready.join("marker")).unwrap(), b"new");
        assert!(!previous.exists());
    }

    #[test]
    fn rejects_wrong_branch_preview_and_downgrade() {
        let engine = Installation {
            path: "engine".into(),
            version: "5.8.3".into(),
            association: "5.8".into(),
        };
        let release = Release {
            app: "UE_5.8".into(),
            build: Some("5.8.4-123+++UE5+Release-5.8-Windows".into()),
            version: EngineVersion::parse("5.8.4"),
            platform: "Windows".into(),
            channel: "Live".into(),
            catalog_id: "".into(),
        };
        validate_release(&engine, &release).unwrap();
        for invalid in [
            Release {
                app: "UE_5.7".into(),
                ..release.clone()
            },
            Release {
                channel: "Preview".into(),
                ..release.clone()
            },
            Release {
                version: EngineVersion::parse("5.8.2"),
                ..release.clone()
            },
            Release {
                build: None,
                ..release.clone()
            },
        ] {
            assert!(validate_release(&engine, &invalid).is_err());
        }
    }

    #[test]
    fn custom_file_collision_aborts_without_changing_original() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("engine");
        let ready = temp.path().join("ready");
        let (old, _) = plan(&original, 3);
        let (mut next, _) = plan(&ready, 4);
        put(&original, "Engine/custom.dat", b"mine");
        put(&ready, "Engine/custom.dat", b"new engine data");
        next.manifest.files.push(File {
            name: "Engine/custom.dat".into(),
            ..Default::default()
        });
        assert!(
            preserve_custom(
                &original,
                &ready,
                &old,
                &next,
                &Control::default(),
                &Mutex::new(Progress::default())
            )
            .is_err()
        );
        assert_eq!(
            fs::read(original.join("Engine/custom.dat")).unwrap(),
            b"mine"
        );
    }
}
