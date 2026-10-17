//! User-initiated updates of the installed edition. Never replace a running executable.
use super::{REPOSITORY, Update};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_INSTALLER: u64 = 512 * 1024 * 1024;

pub struct StagedInstaller {
    directory: PathBuf,
    path: PathBuf,
    update: Update,
    handed_off: bool,
}

impl Drop for StagedInstaller {
    fn drop(&mut self) {
        if !self.handed_off {
            // Only remove our own files, never recursively traverse a staging folder.
            let _ = fs::remove_file(self.directory.join("installer.part"));
            let _ = fs::remove_file(&self.path);
            for name in [
                "update-supervisor.exe",
                "handoff.json",
                "ready",
                "proceed",
                "helper-error.txt",
            ] {
                let _ = fs::remove_file(self.directory.join(name));
            }
            let _ = fs::remove_dir(&self.directory);
        }
    }
}

fn validate_update(update: &Update) -> Result<&str, String> {
    let version = semver::Version::parse(&update.version)
        .map_err(|_| "Invalid launcher update version.".to_string())?;
    let name = format!("DoNothingAndWin-Setup-{version}.exe");
    let valid_url = [version.to_string(), format!("v{version}")]
        .iter()
        .any(|tag| update.download_url == format!("{REPOSITORY}/releases/download/{tag}/{name}"));
    if !version.pre.is_empty()
        || !version.build.is_empty()
        || !valid_url
        || update.size == 0
        || update.size > MAX_INSTALLER
    {
        return Err("Invalid launcher installer details.".into());
    }
    update
        .sha256
        .as_deref()
        .filter(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| {
            "This release has no verified SHA-256 digest. Download it manually from Release notes."
                .into()
        })
}

/// An unpacked development binary must not accidentally become an installed edition.
pub fn installation_directory() -> Result<PathBuf, String> {
    if cfg!(feature = "portable") {
        return Err("Portable editions are updated by downloading the new ZIP.".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = exe
        .parent()
        .ok_or("Could not locate the installed launcher.")?;
    if exe
        .file_name()
        .is_none_or(|name| name != "DoNothingAndWinLauncher.exe")
        || !directory.join("unins000.exe").is_file()
    {
        return Err("Install this launcher using Setup before using in-app updates.".into());
    }
    Ok(directory.to_owned())
}

pub fn download(
    update: Update,
    root: &Path,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<StagedInstaller, String> {
    validate_update(&update)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Launcher update cancelled.".into());
    }
    fs::create_dir_all(root).map_err(|e| format!("Could not stage launcher update: {e}"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let directory = root.join(format!("{}-{}-{nonce}", update.version, std::process::id()));
    fs::create_dir(&directory).map_err(|e| e.to_string())?;
    let staged = StagedInstaller {
        path: directory.join("installer.exe"),
        directory,
        update,
        handed_off: false,
    };
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("DoNothingAndWin/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let allowed = matches!(
                attempt.url().host_str(),
                Some(
                    "github.com"
                        | "release-assets.githubusercontent.com"
                        | "objects.githubusercontent.com"
                )
            );
            if attempt.previous().len() >= 5 || attempt.url().scheme() != "https" || !allowed {
                attempt.error("Unexpected launcher download redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| format!("Could not start launcher download: {e}"))?;
    let mut response = client
        .get(&staged.update.download_url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|_| {
            "Could not download the launcher installer. Check your connection and retry."
                .to_string()
        })?;
    if response
        .content_length()
        .is_some_and(|size| size != staged.update.size)
    {
        return Err("Installer download size does not match the release.".into());
    }
    let partial = staged.directory.join("installer.part");
    let mut file = File::create_new(&partial).map_err(|e| e.to_string())?;
    copy_verified(
        &mut response,
        &mut file,
        &staged.update,
        cancelled,
        &mut progress,
    )?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(&partial, &staged.path).map_err(|e| e.to_string())?;
    Ok(staged)
}

fn copy_verified(
    input: &mut impl Read,
    output: &mut impl Write,
    update: &Update,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(u64),
) -> Result<(), String> {
    let expected = validate_update(update)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Launcher update cancelled.".into());
        }
        let count = input
            .read(&mut buffer)
            .map_err(|e| format!("Installer download interrupted: {e}"))?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > update.size {
            return Err("Installer download exceeds the expected size.".into());
        }
        output
            .write_all(&buffer[..count])
            .map_err(|e| format!("Could not save installer: {e}"))?;
        hash.update(&buffer[..count]);
        progress(total);
    }
    if total != update.size || !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected) {
        return Err("Installer verification failed. Nothing was installed. Please retry.".into());
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Handoff {
    pub directory: PathBuf,
    pub update: Update,
}
pub fn verify_handoff(root: &Path, request: &Handoff) -> Result<File, String> {
    if !request.directory.is_absolute()
        || !request.directory.join("unins000.exe").is_file()
        || !request
            .directory
            .join("DoNothingAndWinLauncher.exe")
            .is_file()
    {
        return Err("The installed launcher location is no longer valid.".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1); // FILE_SHARE_READ; reject writes/deletion until setup exits.
    }
    let mut file = options
        .open(root.join("installer.exe"))
        .map_err(|e| e.to_string())?;
    copy_verified(
        &mut file,
        &mut std::io::sink(),
        &request.update,
        &AtomicBool::new(false),
        &mut |_| {},
    )?;
    Ok(file)
}
impl StagedInstaller {
    /// Runs on a worker after active operations finish. The helper never holds the app lock.
    pub fn launch_supervised(&mut self, cancelled: &AtomicBool) -> Result<(), String> {
        let request = Handoff {
            directory: installation_directory()?,
            update: self.update.clone(),
        };
        let _verified = verify_handoff(&self.directory, &request)?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("Update cancelled before handoff.".into());
        }
        let helper = self.directory.join("update-supervisor.exe");
        fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, &helper)
            .map_err(|e| e.to_string())?;
        fs::write(
            self.directory.join("handoff.json"),
            serde_json::to_vec(&request).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut child = std::process::Command::new(&helper)
            .arg("--supervise-update")
            .current_dir(&self.directory)
            .spawn()
            .map_err(|e| format!("Could not start update helper: {e}"))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            if cancelled.load(Ordering::Relaxed) || std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "Update preparation cancelled or timed out. The launcher is still open.".into(),
                );
            }
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                let detail = fs::read_to_string(self.directory.join("helper-error.txt"))
                    .unwrap_or_else(|_| status.to_string());
                return Err(format!(
                    "Update helper failed: {detail}. The launcher is still open."
                ));
            }
            if self.directory.join("ready").is_file() {
                if let Err(e) = fs::write(self.directory.join("proceed"), b"proceed") {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("Could not hand off update: {e}"));
                }
                self.handed_off = true;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Downloads the published GitHub installer into a temporary folder; never executes it"]
    fn live_installer_download_matches_release_digest() {
        let update = crate::launcher_update::check("0.0.0", false)
            .unwrap()
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let staged = download(update, root.path(), &AtomicBool::new(false), |_| {}).unwrap();
        assert!(staged.path.is_file());
        let directory = staged.directory.clone();
        drop(staged);
        assert!(!directory.exists());
    }
    fn update(bytes: &[u8]) -> Update {
        Update {
            version: "9.0.0".into(),
            release_url: String::new(),
            download_url: format!(
                "{REPOSITORY}/releases/download/v9.0.0/DoNothingAndWin-Setup-9.0.0.exe"
            ),
            size: bytes.len() as u64,
            sha256: Some(format!("{:x}", Sha256::digest(bytes))),
        }
    }
    #[test]
    fn rejects_corruption_truncation_excess_and_cancellation() {
        let expected = update(b"installer");
        for bytes in [b"installeX".as_slice(), b"install", b"installer-extra"] {
            assert!(
                copy_verified(
                    &mut &*bytes,
                    &mut Vec::new(),
                    &expected,
                    &AtomicBool::new(false),
                    &mut |_| {}
                )
                .is_err()
            );
        }
        assert!(
            copy_verified(
                &mut b"installer".as_slice(),
                &mut Vec::new(),
                &expected,
                &AtomicBool::new(true),
                &mut |_| {}
            )
            .is_err()
        );
        let mut output = Vec::new();
        let mut count = 0;
        copy_verified(
            &mut b"installer".as_slice(),
            &mut output,
            &expected,
            &AtomicBool::new(false),
            &mut |n| count = n,
        )
        .unwrap();
        assert_eq!(output, b"installer");
        assert_eq!(count, 9);
    }
    #[test]
    fn refuses_unverifiable_or_foreign_installers() {
        let mut u = update(b"installer");
        u.sha256 = None;
        assert!(validate_update(&u).is_err());
        u = update(b"installer");
        u.download_url = "https://example.com/app.exe".into();
        assert!(validate_update(&u).is_err());
        u = update(b"installer");
        u.size = MAX_INSTALLER + 1;
        assert!(validate_update(&u).is_err());
    }
    #[test]
    fn abandoned_staging_removes_only_its_files() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("stage");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("installer.exe");
        fs::write(&path, b"installer").unwrap();
        fs::write(directory.join("installer.part"), b"partial").unwrap();
        let staged = StagedInstaller {
            directory: directory.clone(),
            path,
            update: update(b"installer"),
            handed_off: false,
        };
        drop(staged);
        assert!(!directory.exists());
        assert!(root.path().exists());
    }
}
