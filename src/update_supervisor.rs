//! Runs from the staging directory, never from the installation being replaced.
use engine_workspace::{
    instance,
    launcher_update::install::{Handoff, verify_handoff},
    storage,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

// Effects stay behind one seam so recovery tests cannot execute a real installer.
trait Effects {
    fn setup(&mut self) -> Result<i32, String>;
    fn reopen(&mut self) -> Result<(), String>;
    fn explain(&mut self, message: &str);
}
fn supervise(effects: &mut impl Effects) {
    let result = effects.setup();
    let outcome = match result {
        Ok(0) => None,
        // Inno Setup cancellation: https://jrsoftware.org/ishelp/topic_setupexitcodes.htm
        Ok(2 | 5) => {
            Some("The launcher update was cancelled. You can retry it manually.".to_owned())
        }
        Ok(code) => Some(format!(
            "The launcher update did not complete (setup exit code {code}). You can retry it manually."
        )),
        Err(error) => Some(format!(
            "The launcher update could not complete: {error}. You can retry it manually."
        )),
    };
    match effects.reopen() {
        Ok(()) => { if let Some(message) = outcome { effects.explain(&message); } }
        Err(error) => effects.explain(&format!("{}\n\nCould not reopen the launcher: {error}. Open its shortcut, or reinstall using the official release installer.", outcome.unwrap_or_else(|| "Setup completed.".into()))),
    }
}

pub fn run() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if exe.file_name().is_none_or(|n| n != "update-supervisor.exe") {
        return Err("The update helper must run from its staging directory.".into());
    }
    let root = exe.parent().ok_or("Missing update directory")?;
    let request: Handoff = serde_json::from_reader(std::io::Read::take(
        std::fs::File::open(root.join("handoff.json")).map_err(|e| e.to_string())?,
        1024 * 1024,
    ))
    .map_err(|e| e.to_string())?;
    // Keep a read-only sharing handle alive from verification through execution.
    let _verified = verify_handoff(root, &request)?;
    std::fs::write(root.join("ready"), b"ready").map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !root.join("proceed").is_file() {
        if Instant::now() >= deadline {
            return Err("Update handoff timed out; setup was not started.".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut effects = WindowsEffects {
        root,
        request: &request,
    };
    supervise(&mut effects);
    Ok(())
}
struct WindowsEffects<'a> {
    root: &'a Path,
    request: &'a Handoff,
}
impl Effects for WindowsEffects<'_> {
    fn setup(&mut self) -> Result<i32, String> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match instance::acquire(&storage::data_dir().join("launcher-instance.lock")) {
                Ok(Some(lock)) => {
                    drop(lock);
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Ok(None) => {
                    return Err(
                        "The launcher did not finish closing; no installation was attempted".into(),
                    );
                }
                Err(e) => return Err(format!("Could not check the launcher lock: {e}")),
            }
        }
        std::process::Command::new(self.root.join("installer.exe"))
            .args([
                "/SILENT",
                "/SP-",
                "/NORESTART",
                "/NOCLOSEAPPLICATIONS",
                "/NOFORCECLOSEAPPLICATIONS",
                "/DNWUPDATE=1",
                "/DNWSUPERVISED=1",
            ])
            .arg(format!("/DIR={}", self.request.directory.display()))
            .arg(format!("/LOG={}", self.root.join("setup.log").display()))
            .current_dir(self.root)
            .status()
            .map_err(|e| e.to_string())?
            .code()
            .ok_or_else(|| "Setup ended without an exit code".into())
    }
    fn reopen(&mut self) -> Result<(), String> {
        let target = self.request.directory.join("DoNothingAndWinLauncher.exe");
        if !target.is_file() {
            return Err("The installed executable is missing".into());
        }
        crate::desktop_launch::launch_installed(&target)?;
        // ShellExecute accepting a request does not prove the app actually started.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match instance::acquire(&storage::data_dir().join("launcher-instance.lock")) {
                Ok(None) => return Ok(()),
                Ok(Some(lock)) => drop(lock),
                Err(e) => return Err(e.to_string()),
            }
            if Instant::now() >= deadline {
                return Err("The launcher did not start successfully".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    fn explain(&mut self, message: &str) {
        // Retain the recovery advice even if the notification is dismissed.
        let _ = std::fs::write(self.root.join("recovery.txt"), message);
        rfd::MessageDialog::new()
            .set_title("Launcher update")
            .set_description(format!(
                "{message}\n\nSetup log: {}",
                self.root.join("setup.log").display()
            ))
            .set_level(rfd::MessageLevel::Warning)
            .show();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        result: Result<i32, String>,
        reopen_error: bool,
        setups: usize,
        reopens: usize,
        messages: Vec<String>,
    }
    impl Effects for Fake {
        fn setup(&mut self) -> Result<i32, String> {
            self.setups += 1;
            self.result.clone()
        }
        fn reopen(&mut self) -> Result<(), String> {
            self.reopens += 1;
            if self.reopen_error {
                Err("missing".into())
            } else {
                Ok(())
            }
        }
        fn explain(&mut self, message: &str) {
            self.messages.push(message.into());
        }
    }
    #[test]
    fn success_failure_cancellation_and_spawn_errors_never_retry() {
        for result in [
            Ok(0),
            Ok(1),
            Ok(2),
            Ok(5),
            Ok(7),
            Err("spawn failed".into()),
        ] {
            for reopen_error in [false, true] {
                let success = result == Ok(0);
                let mut fake = Fake {
                    result: result.clone(),
                    reopen_error,
                    setups: 0,
                    reopens: 0,
                    messages: vec![],
                };
                supervise(&mut fake);
                assert_eq!((fake.setups, fake.reopens), (1, 1));
                assert_eq!(fake.messages.is_empty(), success && !reopen_error);
                if !success {
                    assert_eq!(
                        fake.messages[0].contains("was cancelled"),
                        matches!(result, Ok(2 | 5))
                    );
                }
                if reopen_error {
                    assert!(fake.messages[0].contains("official release installer"));
                }
            }
        }
    }
}
