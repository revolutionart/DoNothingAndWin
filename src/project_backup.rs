//! Full project copies published only after every file has been copied successfully.
use crate::installer;
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Default)]
pub struct Progress {
    pub phase: String,
    pub file: String,
    pub copied: u64,
    pub total: u64,
    pub files_done: usize,
    pub files_total: usize,
}

#[derive(PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    directory: bool,
    size: u64,
    modified: Option<SystemTime>,
}

fn checked_path(path: &Path) -> Result<(), String> {
    installer::no_links(path).map_err(|_| {
        format!(
            "Backup cannot follow a link or junction: {}",
            path.display()
        )
    })
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Backup cancelled.".into())
    } else {
        Ok(())
    }
}

fn inventory(root: &Path, cancel: &AtomicBool) -> Result<Vec<Entry>, String> {
    let mut pending = vec![root.to_owned()];
    let mut entries = Vec::new();
    while let Some(dir) = pending.pop() {
        cancelled(cancel)?;
        checked_path(&dir)?;
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            cancelled(cancel)?;
            let path = entry.map_err(|e| e.to_string())?.path();
            checked_path(&path)?;
            let meta = fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if !meta.is_file() && !meta.is_dir() {
                return Err(format!("Unsupported file: {}", path.display()));
            }
            if meta.is_dir() {
                pending.push(path.clone());
            }
            entries.push(Entry {
                path: path.strip_prefix(root).map_err(|e| e.to_string())?.into(),
                directory: meta.is_dir(),
                size: if meta.is_file() { meta.len() } else { 0 },
                modified: if meta.is_file() {
                    meta.modified().ok()
                } else {
                    None
                },
            });
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// `destination` is a new directory under an existing chosen backup location.
/// Never registers the copy as a project or changes the original descriptor.
pub fn backup_project(
    descriptor: &Path,
    destination: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    cancelled(cancel)?;
    checked_path(descriptor)?;
    let descriptor = fs::canonicalize(descriptor).map_err(|e| e.to_string())?;
    if descriptor.extension().and_then(|s| s.to_str()) != Some("uproject") {
        return Err("Choose an Unreal project file.".into());
    }
    let _: serde_json::Value = serde_json::from_slice(
        &fs::read(&descriptor).map_err(|e| format!("Cannot read project: {e}"))?,
    )
    .map_err(|_| "Invalid project file")?;
    let root = descriptor.parent().ok_or("Project has no folder")?;
    if !destination.is_absolute() {
        return Err("Choose an absolute backup location.".into());
    }
    checked_path(destination)?;
    if destination.exists() {
        return Err("Backup folder already exists. Choose a new folder.".into());
    }
    let parent = fs::canonicalize(destination.parent().ok_or("Invalid backup location")?)
        .map_err(|_| "Backup location must be an existing folder")?;
    let destination = parent.join(destination.file_name().ok_or("Invalid backup folder")?);
    if parent.starts_with(root) || root.starts_with(&destination) {
        return Err("Choose a backup location outside the original project folder.".into());
    }
    let mut state = Progress {
        phase: "Checking project files…".into(),
        ..Default::default()
    };
    progress(state.clone());
    let entries = inventory(root, cancel)?;
    state.total = entries
        .iter()
        .try_fold(0u64, |n, e| n.checked_add(e.size))
        .ok_or("Project size exceeds the supported limit")?;
    state.files_total = entries.iter().filter(|e| !e.directory).count();
    if fs2::available_space(&parent).map_err(|e| e.to_string())?
        < state.total.saturating_add(16 * 1024 * 1024)
    {
        return Err("Not enough free space at the backup location.".into());
    }
    let stage = parent.join(format!(
        ".engine-workspace-backup-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir(&stage).map_err(|e| format!("Cannot create backup folder: {e}"))?;
    let result = (|| {
        let mut buffer = vec![0u8; 1024 * 1024];
        state.phase = "Copying project files…".into();
        for entry in &entries {
            cancelled(cancel)?;
            let input = root.join(&entry.path);
            let output = stage.join(&entry.path);
            checked_path(&input)?;
            checked_path(&output)?;
            if entry.directory {
                fs::create_dir_all(&output).map_err(|e| e.to_string())?;
                continue;
            }
            state.file = entry.path.to_string_lossy().into_owned();
            let before = fs::metadata(&input).map_err(|e| format!("{}: {e}", input.display()))?;
            if before.len() != entry.size || before.modified().ok() != entry.modified {
                return Err("Project changed during backup. Close the editor and retry.".into());
            }
            fs::create_dir_all(output.parent().ok_or("Invalid backup file")?)
                .map_err(|e| e.to_string())?;
            let mut reader = File::open(&input).map_err(|e| format!("{}: {e}", input.display()))?;
            let mut writer =
                File::create_new(&output).map_err(|e| format!("{}: {e}", output.display()))?;
            let mut copied = 0;
            loop {
                cancelled(cancel)?;
                let n = reader
                    .read(&mut buffer)
                    .map_err(|e| format!("{}: {e}", input.display()))?;
                if n == 0 {
                    break;
                }
                writer
                    .write_all(&buffer[..n])
                    .map_err(|e| format!("{}: {e}", output.display()))?;
                copied += n as u64;
                state.copied += n as u64;
                progress(state.clone());
            }
            writer
                .sync_all()
                .map_err(|e| format!("{}: {e}", output.display()))?;
            let after = reader.metadata().map_err(|e| e.to_string())?;
            if copied != entry.size
                || after.len() != entry.size
                || after.modified().ok() != entry.modified
            {
                return Err("Project changed during backup. Close the editor and retry.".into());
            }
            state.files_done += 1;
            progress(state.clone());
        }
        state.phase = "Checking completed copy…".into();
        state.file.clear();
        progress(state.clone());
        if inventory(root, cancel)? != entries {
            return Err("Project changed during backup. Close the editor and retry.".into());
        }
        cancelled(cancel)?;
        checked_path(&destination)?;
        checked_path(&stage)?;
        if destination.exists() {
            return Err("Backup destination appeared during copying; nothing overwritten.".into());
        }
        fs::rename(&stage, &destination).map_err(|e| format!("Cannot finish backup: {e}"))?;
        state.phase = "Backup complete".into();
        progress(state);
        Ok(destination.clone())
    })();
    if stage.exists() {
        // This unique sibling was created by this operation; never remove the destination.
        checked_path(&stage)?;
        fs::remove_dir_all(&stage).map_err(|e| {
            format!(
                "Backup incomplete. Could not remove partial copy {}: {e}",
                stage.display()
            )
        })?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Original");
        fs::create_dir_all(root.join("Content/Empty")).unwrap();
        fs::write(
            root.join("Original.uproject"),
            br#"{"EngineAssociation":"5.8"}"#,
        )
        .unwrap();
        fs::write(
            root.join("Content/large.uasset"),
            vec![7; 3 * 1024 * 1024 + 5],
        )
        .unwrap();
        (temp, root)
    }
    #[test]
    fn backup_preserves_names_bytes_empty_folders_and_reports_incremental_progress() {
        let (temp, root) = fixture();
        let dest = temp.path().join("Backup");
        let mut events = vec![];
        backup_project(
            &root.join("Original.uproject"),
            &dest,
            &AtomicBool::new(false),
            |p| events.push(p),
        )
        .unwrap();
        for path in ["Original.uproject", "Content/large.uasset"] {
            assert_eq!(
                fs::read(root.join(path)).unwrap(),
                fs::read(dest.join(path)).unwrap()
            );
        }
        assert!(dest.join("Content/Empty").is_dir());
        assert!(
            events
                .iter()
                .any(|p| p.copied > 0 && p.copied < p.total && p.files_done == 0)
        );
        let last = events.last().unwrap();
        assert_eq!(last.copied, last.total);
        assert_eq!(last.files_done, 2);
        assert_eq!(last.phase, "Backup complete");
    }
    #[test]
    fn cancellation_and_source_changes_never_publish_partial_backups() {
        for change_source in [false, true] {
            let (temp, root) = fixture();
            let cancel = AtomicBool::new(false);
            let dest = temp.path().join("Backup");
            let mut changed = false;
            let result = backup_project(&root.join("Original.uproject"), &dest, &cancel, |p| {
                if p.copied > 0 && !changed {
                    changed = true;
                    if change_source {
                        fs::write(root.join("New.txt"), b"changed").unwrap();
                    } else {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            });
            assert!(result.is_err());
            assert!(!dest.exists());
            assert!(root.join("Original.uproject").is_file());
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        }
    }
    #[test]
    fn existing_and_nested_destinations_are_rejected() {
        let (temp, root) = fixture();
        let cancel = AtomicBool::new(false);
        for dest in [root.clone(), root.join("Backup"), temp.path().into()] {
            assert!(
                backup_project(&root.join("Original.uproject"), &dest, &cancel, |_| {}).is_err()
            );
        }
        assert!(!root.join("Backup").exists());
    }

    #[test]
    fn destination_created_during_copy_is_preserved_and_staging_is_removed() {
        let (temp, root) = fixture();
        let dest = temp.path().join("Backup");
        let result = backup_project(
            &root.join("Original.uproject"),
            &dest,
            &AtomicBool::new(false),
            |p| {
                if p.copied > 0 && !dest.exists() {
                    fs::create_dir(&dest).unwrap();
                    fs::write(dest.join("existing.txt"), b"keep").unwrap();
                }
            },
        );
        assert!(result.unwrap_err().contains("nothing overwritten"));
        assert_eq!(fs::read(dest.join("existing.txt")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn editing_a_source_file_during_copy_fails_without_publishing() {
        let (temp, root) = fixture();
        let dest = temp.path().join("Backup");
        let mut changed = false;
        let result = backup_project(
            &root.join("Original.uproject"),
            &dest,
            &AtomicBool::new(false),
            |p| {
                if p.copied > 0 && !changed {
                    changed = true;
                    fs::write(root.join("Content/large.uasset"), b"edited").unwrap();
                }
            },
        );
        assert!(result.unwrap_err().contains("Project changed"));
        assert!(!dest.exists());
        assert_eq!(
            fs::read(root.join("Content/large.uasset")).unwrap(),
            b"edited"
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
