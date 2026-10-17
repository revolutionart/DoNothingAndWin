use super::*;
use engine_workspace::launcher_update::{self, Update};
use launcher_update::install::{self, StagedInstaller};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Default)]
pub(super) struct LauncherUpdates {
    receiver: Option<Receiver<Result<Option<Update>, String>>>,
    checked: Option<Instant>,
    pub(super) available: Option<Update>,
    error: Option<String>,
    download: Option<Receiver<Result<StagedInstaller, String>>>,
    cancelled: Arc<AtomicBool>,
    downloaded: Arc<AtomicU64>,
    download_size: u64,
    staged: Option<StagedInstaller>,
    pub(super) applying: bool,
}

impl Drop for LauncherUpdates {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Workspace {
    fn launcher_update_busy(&self) -> bool {
        self.pending.is_some()
            || self.login
            || self.transfer.busy()
            || self.plugin_work.mutating
            || self.library_transfer_status().is_some()
            || self.cloning.busy()
            || self.scan.receiver.is_some()
    }

    fn start_launcher_download(&mut self, update: Update, ctx: &Context) {
        if self.launcher_updates.download.is_some() || self.launcher_updates.staged.is_some() {
            return;
        }
        if let Err(error) = install::installation_directory() {
            self.launcher_updates.error = Some(error);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let downloaded = Arc::new(AtomicU64::new(0));
        self.launcher_updates.cancelled = cancelled.clone();
        self.launcher_updates.downloaded = downloaded.clone();
        self.launcher_updates.download_size = update.size;
        self.launcher_updates.download = Some(rx);
        self.launcher_updates.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = install::download(
                update,
                &storage::data_dir().join("launcher-updates"),
                &cancelled,
                |bytes| {
                    downloaded.store(bytes, Ordering::Relaxed);
                    ctx.request_repaint();
                },
            );
            // Closing the app or cancelling discards the staged executable, even if
            // the final network read finished after cancellation was requested.
            if !cancelled.load(Ordering::Relaxed) {
                let _ = tx.send(result);
            }
            ctx.request_repaint();
        });
    }

    fn poll_launcher_download(&mut self) {
        if let Some(result) = self
            .launcher_updates
            .download
            .as_ref()
            .map(|rx| rx.try_recv())
        {
            match result {
                Ok(result) => {
                    self.launcher_updates.download = None;
                    if !self.launcher_updates.cancelled.load(Ordering::Relaxed) {
                        match result {
                            Ok(staged) => self.launcher_updates.staged = Some(staged),
                            Err(error) => self.launcher_updates.error = Some(error),
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.launcher_updates.download = None;
                    if !self.launcher_updates.cancelled.load(Ordering::Relaxed) {
                        self.launcher_updates.error =
                            Some("Launcher download interrupted. Please retry.".into());
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.launcher_update_busy() || self.launcher_updates.applying {
            return;
        }
        if let Some(mut staged) = self.launcher_updates.staged.take() {
            self.prefs.launch_engine = self.launch_path.clone();
            self.prefs.projects = self.projects.clone();
            match storage::save_preferences(&self.prefs).and_then(|()| staged.launch()) {
                Ok(()) => {
                    self.launcher_updates.applying = true;
                    crate::window_frame::request_exit();
                }
                Err(error) => self.launcher_updates.error = Some(error),
            }
        }
    }

    fn start_launcher_update_check(&mut self, ctx: &Context) {
        if self.launcher_updates.receiver.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.launcher_updates.receiver = Some(rx);
        self.launcher_updates.checked = Some(Instant::now());
        self.launcher_updates.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result =
                launcher_update::check(env!("CARGO_PKG_VERSION"), cfg!(feature = "portable"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_launcher_updates(&mut self, ctx: &Context) {
        if self.demo || self.layout_probe.is_some() {
            return;
        }
        self.poll_launcher_download();
        if let Some(result) = self
            .launcher_updates
            .receiver
            .as_ref()
            .map(|rx| rx.try_recv())
        {
            match result {
                Ok(result) => {
                    self.launcher_updates.receiver = None;
                    match result {
                        Ok(update) => {
                            self.launcher_updates.available = update;
                            self.launcher_updates.error = None;
                        }
                        Err(error) => self.launcher_updates.error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.launcher_updates.receiver = None;
                    self.launcher_updates.error =
                        Some("Launcher update check interrupted. Try again.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.launcher_updates.download.is_none()
            && self.launcher_updates.staged.is_none()
            && self
                .launcher_updates
                .checked
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(24 * 60 * 60))
        {
            self.start_launcher_update_check(ctx);
        }
    }

    pub(super) fn launcher_update_settings(&mut self, ui: &mut Ui, ctx: &Context) {
        ui.heading("Launcher updates");
        ui.label(concat!(
            "DoNothing&Win Launcher ",
            env!("CARGO_PKG_VERSION")
        ));
        ui.label("Checks GitHub at startup and once a day while open. Downloads and installation are started by you.");
        let checking = self.launcher_updates.receiver.is_some();
        let updating =
            self.launcher_updates.download.is_some() || self.launcher_updates.staged.is_some();
        if ui
            .add_enabled(
                !checking && !updating,
                buttons::secondary_button("Check for launcher updates"),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            self.start_launcher_update_check(ctx);
        }
        if checking {
            ui.label("Checking for launcher updates…");
        }
        if let Some(update) = self.launcher_updates.available.clone() {
            ui.label(format!("Version {} is available.", update.version));
            let mut open = None;
            let mut download = false;
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !updating,
                        buttons::primary_button(
                            if cfg!(feature = "portable") {
                                "Download portable ZIP"
                            } else {
                                "Update launcher"
                            },
                            &self.theme,
                        ),
                    )
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    if cfg!(feature = "portable") {
                        open = Some(update.download_url.clone());
                    } else {
                        download = true;
                    }
                }
                if ui
                    .add(buttons::secondary_button("Release notes"))
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    open = Some(update.release_url.clone());
                }
            });
            if download {
                self.start_launcher_download(update, ctx);
            }
            if !cfg!(feature = "portable") {
                ui.label("Downloads and verifies the installer, then installs and restarts the launcher. Active work finishes first.");
            }
            if let Some(url) = open {
                if let Err(error) = platform::open_external(&url) {
                    self.notify(format!("Could not open your browser: {error}"));
                }
            }
        } else if !checking
            && self.launcher_updates.error.is_none()
            && self.launcher_updates.checked.is_some()
        {
            ui.label("You’re using the latest launcher version.");
        }
        if updating {
            if self.launcher_updates.staged.is_some() {
                ui.label(
                    "Update verified. Waiting for active operations to finish before restarting...",
                );
            } else {
                let received = self.launcher_updates.downloaded.load(Ordering::Relaxed);
                let total = self.launcher_updates.download_size.max(1);
                ui.add(
                    egui::ProgressBar::new(received as f32 / total as f32).text(format!(
                        "Downloading launcher: {:.1} / {:.1} MB",
                        received as f64 / 1_000_000.,
                        total as f64 / 1_000_000.
                    )),
                );
            }
            if ui
                .add(buttons::secondary_button("Cancel launcher update"))
                .clicked()
            {
                self.launcher_updates
                    .cancelled
                    .store(true, Ordering::Relaxed);
                self.launcher_updates.download = None;
                self.launcher_updates.staged = None;
            }
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if let Some(error) = &self.launcher_updates.error {
            ui.label(RichText::new(error).color(color(self.theme.warning)));
        }
    }
}
