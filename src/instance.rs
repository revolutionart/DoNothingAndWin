use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

/// The OS releases this lock even if the launcher crashes. Every build shares it.
pub fn acquire(path: &Path) -> std::io::Result<Option<File>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exclusive_until_owner_exits_and_stale_file_is_reusable() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("launcher.lock");
        let owner = acquire(&p).unwrap().unwrap();
        assert!(acquire(&p).unwrap().is_none());
        drop(owner);
        assert!(acquire(&p).unwrap().is_some());
    }
}
