use std::path::Path;
use windows::{
    Win32::{
        System::{
            Com::{
                CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Variant::VARIANT,
        },
        UI::Shell::{
            IShellDispatch2, IShellFolderViewDual, IShellWindows, IWebBrowser2, SWC_DESKTOP,
            ShellWindows,
        },
    },
    core::{BSTR, Interface},
};

pub fn relaunch() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    // Do not forward the handoff flag: the desktop starts one normal instance.
    launch(&executable, "", 1).map_err(|e| e.to_string())
}

fn launch(executable: &Path, arguments: &str, show: i32) -> windows::core::Result<()> {
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    let _apartment = Apartment;
    // Obtain the running desktop's dispatch, not an in-process Shell.Application.
    // This keeps Setup's RedirectionGuard in Setup and uses Explorer's normal
    // process context without disabling any system or installer mitigations.
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
        // VT_UI4 means a shell window type; VT_I4 would select collection index 8.
        let desktop: IWebBrowser2 = windows.Item(&VARIANT::from(SWC_DESKTOP.0 as u32))?.cast()?;
        let view: IShellFolderViewDual = desktop.Document()?.cast()?;
        let shell: IShellDispatch2 = view.Application()?.cast()?;
        shell.ShellExecute(
            &BSTR::from(executable.as_os_str().to_string_lossy().as_ref()),
            &VARIANT::from(arguments),
            &VARIANT::from(
                executable
                    .parent()
                    .unwrap_or(Path::new(""))
                    .as_os_str()
                    .to_string_lossy()
                    .as_ref(),
            ),
            &VARIANT::from("open"),
            &VARIANT::from(show),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Requires an interactive Windows desktop; run scripts/test-postinstall-launch.ps1"]
    fn desktop_handoff_does_not_inherit_installer_restrictions() {
        use windows_sys::Win32::System::Threading::{
            ProcessRedirectionTrustPolicy, SetProcessMitigationPolicy,
        };
        let folder = std::path::PathBuf::from(
            std::env::var_os("DNW_LAUNCH_PROBE_DIR").expect("probe directory"),
        );
        let executable = folder.join("LaunchProbe.exe");
        let junction = folder.join("cache junction");
        let direct = folder.join("direct.txt");
        let desktop = folder.join("desktop.txt");
        // This test runs alone in a disposable test process, never in the launcher.
        let flags = 1u32;
        assert_ne!(
            unsafe {
                SetProcessMitigationPolicy(
                    ProcessRedirectionTrustPolicy,
                    (&flags as *const u32).cast(),
                    4,
                )
            },
            0
        );
        assert!(
            std::process::Command::new(&executable)
                .arg(&direct)
                .arg(&junction)
                .status()
                .unwrap()
                .success()
        );
        let baseline = std::fs::read_to_string(&direct).unwrap();
        assert!(baseline.contains("guard=1"), "{baseline}");
        assert!(
            baseline.contains("write_error=448"),
            "must reproduce the original failure: {baseline}"
        );

        launch(
            &executable,
            &format!("\"{}\" \"{}\"", desktop.display(), junction.display()),
            0,
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let result = loop {
            if let Ok(result) = std::fs::read_to_string(&desktop) {
                if result.contains("complete") {
                    break result;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "desktop probe did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        assert!(result.contains("guard=0"), "{result}");
        assert!(result.contains("write=ok"), "{result}");
        let child = std::fs::read_to_string(folder.join("desktop.txt.child")).unwrap();
        assert!(
            child.contains("guard=0") && child.contains("write=ok"),
            "{child}"
        );
    }
}
