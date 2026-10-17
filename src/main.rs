#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[cfg(windows)]
mod desktop_launch;
mod event_loop;
mod ui;
mod window_frame;
fn main() -> eframe::Result {
    #[cfg(windows)]
    if std::env::args().any(|a| a == "--launch-from-desktop") {
        if let Err(message) = desktop_launch::relaunch() {
            rfd::MessageDialog::new()
                .set_title("Installation complete")
                .set_description(format!(
                    "The launcher was installed successfully. Open it from the Start menu or desktop shortcut.\n\nAutomatic launch failed: {message}"
                ))
                .set_level(rfd::MessageLevel::Warning)
                .show();
        }
        return Ok(());
    }
    if std::env::args().any(|a| a == "--epic-login") {
        let exit = match engine_workspace::login::run() {
            Ok(true) => 0,
            Ok(false) => 2,
            Err(message) => {
                rfd::MessageDialog::new()
                    .set_title("Epic sign-in")
                    .set_description(&message)
                    .set_level(rfd::MessageLevel::Error)
                    .show();
                3
            }
        };
        std::process::exit(exit);
    }
    let lock_path = engine_workspace::storage::data_dir().join("launcher-instance.lock");
    // Installer probe: use the same lock as every launcher build, without opening UI.
    if std::env::args().any(|a| a == "--check-running") {
        let code = match engine_workspace::instance::acquire(&lock_path) {
            Ok(Some(_)) => 0,
            Ok(None) => 2,
            Err(_) => 3,
        };
        std::process::exit(code);
    }
    let minimized = std::env::args().any(|a| a == "--minimized");
    let _instance = match engine_workspace::instance::acquire(&lock_path) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            let replace = std::env::args().any(|a| a == "--replace-if-idle");
            if minimized && !replace {
                return Ok(());
            }
            window_frame::activate_existing(replace);
            if !replace {
                return Ok(());
            }
            let mut acquired = None;
            for _ in 0..80 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if let Ok(Some(lock)) = engine_workspace::instance::acquire(&lock_path) {
                    acquired = Some(lock);
                    break;
                }
            }
            match acquired {
                Some(lock) => lock,
                None => return Ok(()),
            }
        }
        Err(e) => {
            rfd::MessageDialog::new()
                .set_title("DoNothing&Win Launcher")
                .set_description(format!("Unable to check the running launcher: {e}"))
                .set_level(rfd::MessageLevel::Error)
                .show();
            return Ok(());
        }
    };
    let demo = false;
    let light = std::env::args().any(|a| a == "--light");
    let preferences = engine_workspace::storage::load_preferences();
    let window_size = std::env::args()
        .find_map(|a| {
            let size = a.strip_prefix("--window-size=")?;
            let (w, h) = size.split_once('x')?;
            Some([
                w.parse::<f32>().ok()?.clamp(1000., 7680.),
                h.parse::<f32>().ok()?.clamp(680., 4320.),
            ])
        })
        .or(preferences
            .window_size
            .filter(|s| s.iter().all(|v| v.is_finite() && *v > 0.)))
        .unwrap_or([1586., 992.]);
    let icon = image::load_from_memory(include_bytes!("../assets/brand_icon.png"))
        .expect("Bundled brand icon must be a valid PNG")
        .to_rgba8();
    let side = icon.width().max(icon.height());
    let mut square = image::RgbaImage::new(side, side);
    image::imageops::overlay(
        &mut square,
        &icon,
        ((side - icon.width()) / 2) as i64,
        ((side - icon.height()) / 2) as i64,
    );
    let icon = eframe::egui::IconData {
        width: side,
        height: side,
        rgba: square.into_raw(),
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size(window_size)
            .with_maximized(preferences.window_maximized)
            .with_icon(icon)
            .with_min_inner_size([1000.0, 680.0])
            .with_decorations(false)
            .with_resizable(true)
            .with_title("DoNothing&Win Launcher"),
        ..Default::default()
    };
    event_loop::run(
        "DoNothing&Win Launcher",
        options,
        Box::new(move |cc| {
            window_frame::install(cc);
            let app = ui::Workspace::new(cc, demo, light);
            if minimized {
                cc.egui_ctx
                    .send_viewport_cmd(eframe::egui::ViewportCommand::Minimized(true));
            }
            Ok(Box::new(app))
        }),
    )
}
