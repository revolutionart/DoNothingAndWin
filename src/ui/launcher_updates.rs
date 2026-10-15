use super::*;
use engine_workspace::launcher_update::{self, Update};

#[derive(Default)]
pub(super) struct LauncherUpdates {
    receiver: Option<Receiver<Result<Option<Update>, String>>>,
    checked: Option<Instant>,
    pub(super) available: Option<Update>,
    error: Option<String>,
}

impl Workspace {
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
        if self
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
        if ui
            .add_enabled(!checking, Button::new("Check for launcher updates"))
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            self.start_launcher_update_check(ctx);
        }
        if checking {
            ui.label("Checking for launcher updates…");
        }
        if let Some(update) = &self.launcher_updates.available {
            ui.label(format!("Version {} is available.", update.version));
            let mut open = None;
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(if cfg!(feature = "portable") {
                        "Download portable ZIP"
                    } else {
                        "Download installer"
                    })
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    open = Some(update.download_url.clone());
                }
                if ui
                    .button("Release notes")
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    open = Some(update.release_url.clone());
                }
            });
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
        if let Some(error) = &self.launcher_updates.error {
            ui.label(RichText::new(error).color(color(self.theme.warning)));
        }
    }
}
