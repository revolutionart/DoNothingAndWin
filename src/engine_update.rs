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

pub struct InstalledBuild {
    pub id: String,
    pub preview: bool,
}

pub fn installed_build(root: &Path) -> Option<InstalledBuild> {
    let id = maintenance::promoted_engine_build(root)?;
    let preview = crate::preview_builds::is_preview(&id);
    Some(InstalledBuild { id, preview })
}

/// Steps of an update, as shown beside each phase:
/// 1 check local files, 2 download, 3 reconstruct, 4 final verification,
/// 5 check prepared files, 6 preserve plugins and custom files,
/// 7 apply and register, 8 remove the previous engine.
pub const STEPS: u8 = 8;

const CHANGED: &str = "Another program changed this engine while its update was being prepared, so the update stopped and left the engine as it is. If the Epic Games Launcher is updating this engine, let it finish. Then refresh engines and retry.";

/// The engine in the folder is still the build this update started from.
fn unchanged(
    original: &Path,
    engine: &Installation,
    installed: Option<&InstalledBuild>,
) -> Result<(), String> {
    let same = crate::platform::inspect_engine(original, "")
        .is_some_and(|current| current.version == engine.version)
        && installed.is_none_or(|build| {
            maintenance::promoted_engine_build(original).as_deref() == Some(&build.id)
        });
    if !same {
        return Err(CHANGED.into());
    }
    crate::epic_registration::ensure_idle(original)
}

/// Remove staging that an earlier update of this engine left for another build.
/// The backup of a previous engine is never removed here.
fn remove_stale_staging(parent: &Path, work: &Path, original: &Path, app: &str) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == work
            || !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".dnw-update-")
            || !installer::entry_is_dir(&entry).unwrap_or(false)
            || installer::leaf_metadata(&path.join("previous")).map_or(true, |m| m.is_some())
        {
            continue;
        }
        let marker: Option<serde_json::Value> = storage::read_json(&path.join("update.json"));
        let ours = match &marker {
            Some(marker) => marker["engine"].as_str().map(Path::new) == Some(original),
            // Staging from before the marker existed records only the engine branch,
            // in the finished copy or in the plan of an unfinished one.
            None => fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|child| child.path())
                .flat_map(|child| {
                    [
                        child.join(".engine-workspace-install.json"),
                        child.join("plan.json"),
                    ]
                })
                .filter_map(|record| storage::read_json::<serde_json::Value>(&record))
                .any(|record| record["app"] == app),
        };
        if ours {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

pub fn run(
    engine: &Installation,
    release: &Release,
    session: &Session,
    snapshot: &Snapshot,
    control: &Control,
    progress: Arc<Mutex<Progress>>,
) -> Result<String, String> {
    crate::download_policy::check_install_allowed()?;
    installer::no_links(Path::new(&engine.path))?;
    let original = fs::canonicalize(&engine.path).map_err(|e| e.to_string())?;
    let _lock = maintenance::lock(&original)?;
    maintenance::ensure_closed(&original)?;
    let installed = installed_build(&original);
    validate_release(engine, installed.as_ref(), release)?;
    crate::epic_registration::ensure_idle(&original)?;
    control.check()?;
    {
        let mut p = progress.lock().unwrap();
        p.steps = STEPS;
        p.begin(1, "Loading installed components and update manifest");
    }
    let old = maintenance::prepare(engine, Some(session), snapshot)?;
    if installed
        .as_ref()
        .is_some_and(|build| old.manifest.build != build.id)
    {
        return Err("The installed engine and its saved manifest have different builds. Refresh engines and retry.".into());
    }
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
    {
        let mut p = progress.lock().unwrap();
        p.begin(1, "Removing files left by an earlier update");
        p.detail = "Only unused staging for another build is removed".into();
    }
    remove_stale_staging(original.parent().unwrap(), &work, &original, &release.app);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    storage::write_json(
        &work.join("update.json"),
        &serde_json::json!({"engine": original, "build": next.manifest.build}),
    )?;
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
    // Staging can take long enough for another launcher to start its own update.
    unchanged(&original, engine, installed.as_ref())?;
    // A file that appears in the folder mid-update looks like a custom file. Report
    // the real cause when the engine itself changed underneath the walk.
    preserve_custom(&original, &ready, &old, &next, control, &progress).map_err(|error| {
        match unchanged(&original, engine, installed.as_ref()) {
            Err(changed) if control.check().is_ok() => changed,
            _ => error,
        }
    })?;
    control.check()?;
    maintenance::ensure_closed(&original)?;
    unchanged(&original, engine, installed.as_ref())?;
    // Do not interrupt these two renames: cancellation before commit leaves the
    // original untouched. A failed second rename restores the original folder.
    progress.lock().unwrap().committed = true;
    phase(&progress, 7, "Applying verified engine update");
    commit(&original, &ready, &previous, |from, to| {
        fs::rename(from, to)
    })?;
    phase(
        &progress,
        7,
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
    phase(&progress, 8, "Removing previous engine backup");
    let cleanup = fs::remove_dir_all(&previous).err();
    if cleanup.is_none() {
        // Only this update's plan, lock and marker files remain.
        let _ = fs::remove_dir_all(&work);
    }
    phase(&progress, 8, "Engine update complete");
    let mut message = format!("Updated Unreal Engine to {}.", release.version_label());
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

pub fn validate_release(
    engine: &Installation,
    build: Option<&InstalledBuild>,
    release: &Release,
) -> Result<(), String> {
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
        || version < &installed
        || release
            .build
            .as_deref()
            .and_then(EngineVersion::parse)
            .as_ref()
            != Some(version)
    {
        return Err("This release is not a compatible update for the installed engine.".into());
    }
    if release.preview || version == &installed {
        let current = build
            .filter(|build| build.preview)
            .and_then(|build| changelist(&build.id));
        let next = release.build.as_deref().and_then(changelist);
        if !current
            .zip(next)
            .is_some_and(|(current, next)| next > current)
        {
            return Err("This release is not newer than the installed preview, or the installed build could not be confirmed as a preview.".into());
        }
    }
    Ok(())
}

fn changelist(build: &str) -> Option<u64> {
    let (version_build, _) = build.strip_suffix("-Windows")?.split_once("+++")?;
    let (_, changelist) = version_build.split_once('-')?;
    changelist.parse().ok().filter(|value| *value > 0)
}

// Byte percentages belong to one measured phase, not the entire update.
fn phase(progress: &Mutex<Progress>, step: u8, name: &str) {
    progress.lock().unwrap().begin(step, name);
}

fn verify_staged(plan: &Plan, control: &Control, progress: &Mutex<Progress>) -> Result<(), String> {
    phase(progress, 4, "Final verification of updated engine files");
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
    let result = installer::for_engine_files(&files, control, |_, file| {
        let path = plan.target.join(installer::relative(&file.name)?);
        if !installer::checked_path_metadata(&path)?
            .is_some_and(|meta| meta.is_file() && meta.len() == file.size())
        {
            return Err(format!("Staged update failed verification: {}", file.name));
        }
        let mut read = 0;
        let hash = installer::hash_file_progress(&path, &mut |bytes| {
            control.check()?;
            read += bytes;
            Ok(())
        })?;
        let mut p = progress.lock().unwrap();
        p.read += read;
        if hash != file.sha {
            return Err(format!("Staged update failed verification: {}", file.name));
        }
        p.completed += read;
        p.files += 1;
        p.detail.clone_from(&file.name);
        Ok(())
    });
    if let Err(error) = &result {
        progress.lock().unwrap().detail.clone_from(error);
    }
    result
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
    // Both walks start at folders whose whole path was checked for links, and
    // refuse any link they meet, so each entry needs only its own listing data.
    installer::no_links(ready)?;
    installer::no_links(original)?;
    phase(progress, 5, "Checking prepared update files");
    progress.lock().unwrap().files_total = wanted.len();
    let mut staged_dirs = vec![ready.to_path_buf()];
    while let Some(directory) = staged_dirs.pop() {
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            control.check()?;
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if installer::entry_is_dir(&entry)? {
                staged_dirs.push(path);
                continue;
            }
            let relative = path.strip_prefix(ready).map_err(|e| e.to_string())?;
            let name = relative.to_string_lossy().replace('\\', "/").to_lowercase();
            {
                let mut p = progress.lock().unwrap();
                // Files kept by an earlier attempt can exceed the update's own count.
                p.files = (p.files + 1).min(p.files_total);
                p.detail = relative.display().to_string();
            }
            if !wanted.contains(&name)
                && !matches!(
                    name.as_str(),
                    ".engine-workspace-install.json" | ".engine-workspace-manifest.json.gz"
                )
            {
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
    }
    // The installed folder holds an unknown number of plugin and custom files,
    // so this step reports a running count instead of a share of a guessed total.
    phase(progress, 6, "Preserving installed plugins and custom files");
    let mut directories = vec![original.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            control.check()?;
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let is_dir = installer::entry_is_dir(&entry)?;
            let relative = path.strip_prefix(original).map_err(|e| e.to_string())?;
            let name = relative.to_string_lossy().replace('\\', "/").to_lowercase();
            if !is_dir {
                let mut p = progress.lock().unwrap();
                p.files += 1;
                p.detail = format!("{} files checked • {}", p.files, relative.display());
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
            let staged = installer::leaf_metadata(&destination)?;
            if is_dir {
                if staged.is_none() {
                    fs::create_dir(&destination).map_err(|e| e.to_string())?;
                }
                directories.push(path);
            } else if !owned.contains(&name) {
                if wanted.contains(&name) {
                    return Err(format!(
                        "This file is not part of the installed engine build, and the update contains a file with the same name: {}. Move or rename your file, then retry the update.",
                        relative.display()
                    ));
                }
                if staged.is_some() {
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

    fn plan(target: &Path, minor: u32, patch: u32, changelist: u32) -> (Plan, Vec<u8>) {
        let version = format!(
            r#"{{"MajorVersion":5,"MinorVersion":{minor},"PatchVersion":{patch},"Changelist":{changelist},"IsLicenseeVersion":0,"IsPromotedBuild":1,"BranchName":"++UE5+Release-5.{minor}"}}"#
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
                    app: format!("UE_5.{minor}"),
                    build: format!(
                        "5.{minor}.{patch}-{changelist}+++UE5+Release-5.{minor}-Windows"
                    ),
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
        assert_staged_update(8, (3, 123), (4, 124));
    }

    #[test]
    fn staged_preview_update_replaces_the_exact_build_in_the_original_folder() {
        assert_staged_update(9, (0, 60351404), (0, 60738873));
    }

    fn assert_staged_update(
        minor: u32,
        (old_patch, old_cl): (u32, u32),
        (new_patch, new_cl): (u32, u32),
    ) {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join(format!("UE_5.{minor}"));
        let ready = temp.path().join("ready");
        let (mut old, old_data) = plan(&original, minor, old_patch, old_cl);
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
        let (staged, data) = plan(&ready, minor, new_patch, new_cl);
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
            maintenance::promoted_engine_build(&original).as_deref(),
            Some(old.manifest.build.as_str())
        );
        installer::stage_update(&staged, &original, &Control::default(), progress.clone()).unwrap();
        assert_eq!(
            progress.lock().unwrap().phase,
            "Update files prepared - final verification pending"
        );
        assert_eq!(progress.lock().unwrap().total, 0);
        assert_eq!(
            progress.lock().unwrap().read,
            2 * data.len() as u64 + old_data.len() as u64,
            "Only one staging hash of the reused editor is needed before final verification"
        );
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
            maintenance::promoted_engine_build(&original).as_deref(),
            Some(old.manifest.build.as_str())
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
            maintenance::promoted_engine_build(&original).as_deref(),
            Some(staged.manifest.build.as_str())
        );
        assert!(!ready.exists());
        assert_eq!(
            fs::read(original.join("Engine/Plugins/Marketplace/Custom/plugin.dll")).unwrap(),
            b"my plugin"
        );
        assert!(!original.join("Engine/obsolete.dat").exists());
        assert_eq!(
            maintenance::promoted_engine_build(&backup).as_deref(),
            Some(old.manifest.build.as_str())
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
            assert!(p.detail.contains("Engine/Build/Build.version"));
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
    fn only_unused_staging_of_this_engine_is_removed() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path();
        let original = parent.join("UE_5.9");
        let staging = |name: &str, marker: Option<&Path>, app: &str, backup: bool| {
            let dir = parent.join(name);
            put(&dir, "ready/Engine/file.dat", b"staged");
            storage::write_json(
                &dir.join("ready/.engine-workspace-install.json"),
                &serde_json::json!({"app": app}),
            )
            .unwrap();
            if let Some(engine) = marker {
                storage::write_json(
                    &dir.join("update.json"),
                    &serde_json::json!({"engine": engine}),
                )
                .unwrap();
            }
            if backup {
                put(&dir, "previous/Engine/file.dat", b"previous engine");
            }
            dir
        };
        let current = staging(".dnw-update-current", Some(&original), "UE_5.9", false);
        let older = staging(".dnw-update-older", Some(&original), "UE_5.9", false);
        let legacy = staging(".dnw-update-legacy", None, "UE_5.9", false);
        let unfinished = parent.join(".dnw-update-unfinished");
        put(
            &unfinished,
            ".engine-workspace-abc/files/Engine/a.dat",
            b"x",
        );
        storage::write_json(
            &unfinished.join(".engine-workspace-abc/plan.json"),
            &serde_json::json!({"app": "UE_5.9"}),
        )
        .unwrap();
        let other_engine = staging(
            ".dnw-update-other-engine",
            Some(&parent.join("UE_5.9-copy")),
            "UE_5.9",
            false,
        );
        let other_branch = staging(".dnw-update-other-branch", None, "UE_5.8", false);
        let backup = staging(".dnw-update-backup", Some(&original), "UE_5.9", true);
        let unrelated = parent.join("MyProject");
        put(
            &unrelated,
            "ready/.engine-workspace-install.json",
            b"{\"app\":\"UE_5.9\"}",
        );

        remove_stale_staging(parent, &current, &original, "UE_5.9");

        for removed in [&older, &legacy, &unfinished] {
            assert!(!removed.exists(), "{}", removed.display());
        }
        for kept in [&current, &other_engine, &other_branch, &backup, &unrelated] {
            assert!(kept.exists(), "{}", kept.display());
        }
        assert_eq!(
            fs::read(backup.join("previous/Engine/file.dat")).unwrap(),
            b"previous engine"
        );
    }

    #[test]
    fn preview_revision_at_the_same_version_can_update_in_place() {
        let engine = Installation {
            path: "engine".into(),
            version: "5.9.0".into(),
            association: "5.9".into(),
        };
        let release = Release {
            preview_label: None,
            app: "UE_5.9".into(),
            build: Some("5.9.0-60738873+++UE5+Release-5.9-Windows".into()),
            version: EngineVersion::parse("5.9.0"),
            platform: "Windows".into(),
            preview: true,
            channel: "Live".into(),
            catalog_id: String::new(),
        };
        let preview_1 = InstalledBuild {
            id: "5.9.0-60351404+++UE5+Release-5.9-Windows".into(),
            preview: true,
        };
        validate_release(&engine, Some(&preview_1), &release).unwrap();
        let preview_2 = InstalledBuild {
            id: release.build.clone().unwrap(),
            preview: true,
        };
        assert!(validate_release(&engine, Some(&preview_2), &release).is_err());
        assert!(validate_release(&engine, None, &release).is_err());
        let stable = InstalledBuild {
            preview: false,
            ..preview_1
        };
        assert!(validate_release(&engine, Some(&stable), &release).is_err());
        let future = Release {
            build: Some("5.9.0-61000000+++UE5+Release-5.9-Windows".into()),
            ..release.clone()
        };
        assert!(validate_release(&engine, Some(&preview_2), &future).is_ok());
        let older = Release {
            build: Some(stable.id.clone()),
            ..release
        };
        assert!(validate_release(&engine, Some(&preview_2), &older).is_err());
    }

    #[test]
    fn rejects_wrong_branch_preview_and_downgrade() {
        let engine = Installation {
            path: "engine".into(),
            version: "5.8.3".into(),
            association: "5.8".into(),
        };
        let release = Release {
            preview_label: None,
            app: "UE_5.8".into(),
            build: Some("5.8.4-123+++UE5+Release-5.8-Windows".into()),
            version: EngineVersion::parse("5.8.4"),
            platform: "Windows".into(),
            preview: false,
            channel: "Live".into(),
            catalog_id: "".into(),
        };
        validate_release(&engine, None, &release).unwrap();
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
            assert!(validate_release(&engine, None, &invalid).is_err());
        }
    }

    #[test]
    fn custom_file_collision_aborts_without_changing_original() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("engine");
        let ready = temp.path().join("ready");
        let (old, _) = plan(&original, 8, 3, 123);
        let (mut next, _) = plan(&ready, 8, 4, 124);
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
