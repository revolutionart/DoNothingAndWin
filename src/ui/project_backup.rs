use super::*;
use engine_workspace::project_backup::{self, Progress};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

enum Event {
    Progress(Progress),
    Done(Result<PathBuf, String>),
}

#[derive(Default)]
pub(super) struct Backup {
    pub(super) open: bool,
    source: Option<Project>,
    location: String,
    folder_name: String,
    progress: Progress,
    error: Option<String>,
    completed: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<Event>>,
    preview: bool,
}
impl Backup {
    pub(super) fn busy(&self) -> bool {
        self.rx.is_some() || self.preview
    }
    pub(super) fn fraction(&self) -> Option<f32> {
        (self.progress.total > 0)
            .then(|| (self.progress.copied as f32 / self.progress.total as f32).clamp(0., 1.))
    }
    pub(super) fn status(&self) -> String {
        self.progress.phase.clone()
    }
    fn next_event(&mut self) -> Option<Event> {
        match self.rx.as_ref()?.try_recv() {
            Ok(event) => {
                if matches!(event, Event::Done(_)) {
                    self.rx = None;
                }
                Some(event)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.rx = None;
                Some(Event::Done(Err(
                    "Backup worker stopped before completion. Please retry.".into(),
                )))
            }
        }
    }
}

impl Workspace {
    pub(super) fn begin_project_backup(&mut self, source: Project) {
        if self.backup.busy() {
            self.backup.open = true;
            return;
        }
        let now = time::OffsetDateTime::now_utc();
        let name = Path::new(&source.path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let folder_name = format!(
            "{name}_Backup_{}_{:02}-{:02}-{:02}",
            now.date(),
            now.hour(),
            now.minute(),
            now.second()
        );
        self.backup = Backup {
            open: true,
            source: Some(source),
            folder_name,
            ..Default::default()
        };
    }

    pub(super) fn poll_project_backup(&mut self, ctx: &Context) {
        while let Some(event) = self.backup.next_event() {
            match event {
                Event::Progress(p) => self.backup.progress = p,
                Event::Done(result) => {
                    match result {
                        Ok(path) => {
                            self.notify(format!(
                                "Project backup complete: {}",
                                dialog::display_path(&path.to_string_lossy())
                            ));
                            self.backup.completed = Some(path);
                        }
                        Err(error) => {
                            self.backup.error = Some(error.clone());
                            self.notify(error);
                        }
                    }
                    self.backup.open = true;
                }
            }
        }
        if self.backup.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    pub(super) fn project_backup_dialog(&mut self, ctx: &Context) {
        if !self.backup.open {
            return;
        }
        let Some(source) = self.backup.source.clone() else {
            return;
        };
        let mut open = true;
        let mut start = false;
        let mut close = false;
        let busy = self.backup.busy();
        let done = self.backup.completed.is_some();
        dialog::show(
            ctx,
            &self.theme.clone(),
            "backup-project",
            "Backup project",
            "Save a full copy of this project in a new backup folder.",
            &mut open,
            |ui| {
                dialog::info(ui, &self.theme, &source.name, &source.path);
                ui.add_enabled_ui(!busy && !done, |ui| {
                    ui.label("Backup location");
                    dialog::folder(ui, &mut self.backup.location);
                });
                if !self.backup.location.is_empty() {
                    dialog::info(
                        ui,
                        &self.theme,
                        "Backup folder",
                        &Path::new(&self.backup.location)
                            .join(&self.backup.folder_name)
                            .to_string_lossy(),
                    );
                }
                if busy || done {
                    ui.label(&self.backup.progress.phase);
                    if let Some(fraction) = self.backup.fraction() {
                        ui.add(ProgressBar::new(fraction).show_percentage().animate(busy));
                        ui.label(format!(
                            "{:.1} / {:.1} MB • {} / {} files",
                            self.backup.progress.copied as f64 / 1_048_576.,
                            self.backup.progress.total as f64 / 1_048_576.,
                            self.backup.progress.files_done,
                            self.backup.progress.files_total
                        ));
                    } else {
                        ui.spinner();
                    }
                    if !self.backup.progress.file.is_empty() {
                        ui.add(Label::new(&self.backup.progress.file).truncate())
                            .on_hover_text(&self.backup.progress.file);
                    }
                }
                if let Some(error) = &self.backup.error {
                    ui.colored_label(color(self.theme.warning), error);
                }
                if busy {
                    ui.label("You can close this dialog; the backup will continue. Click the progress at the bottom to reopen it.");
                    let cancelling = self.backup.cancel.load(Ordering::Relaxed);
                    if ui
                        .add_enabled(
                            !cancelling,
                            buttons::secondary_button(if cancelling {
                                "Cancelling…"
                            } else {
                                "Cancel backup"
                            }),
                        )
                        .clicked()
                    {
                        self.backup.cancel.store(true, Ordering::Relaxed);
                    }
                } else if let Some(path) = &self.backup.completed {
                    ui.separator();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(primary_button("Done", &self.theme)).clicked() {
                            close = true;
                        }
                        if ui
                            .add(buttons::secondary_button("Open backup folder"))
                            .clicked()
                        {
                            if let Err(error) = platform::open_external(&path.to_string_lossy()) {
                                self.backup.error = Some(error);
                            }
                        }
                    });
                } else {
                    ui.label("Includes all project files and subfolders. Close the project in Unreal Editor before making a backup.");
                    start = dialog::footer(
                        ui,
                        &self.theme,
                        "Make Backup",
                        !self.demo && !self.backup.location.is_empty(),
                        false,
                    );
                }
            },
        );
        self.backup.open = open && !close;
        if start {
            let destination = Path::new(&self.backup.location).join(&self.backup.folder_name);
            let (tx, rx) = mpsc::channel();
            self.backup.rx = Some(rx);
            self.backup.error = None;
            self.backup.progress = Progress {
                phase: "Checking project files…".into(),
                ..Default::default()
            };
            self.backup.cancel = Arc::new(AtomicBool::new(false));
            let cancel = self.backup.cancel.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut phase = String::new();
                let result = project_backup::backup_project(
                    Path::new(&source.path),
                    &destination,
                    &cancel,
                    |p| {
                        if last.elapsed() >= Duration::from_millis(100) || phase != p.phase {
                            phase = p.phase.clone();
                            let _ = tx.send(Event::Progress(p));
                            last = Instant::now();
                            ctx.request_repaint();
                        }
                    },
                );
                let _ = tx.send(Event::Done(result));
                ctx.request_repaint();
            });
        }
    }

    pub(super) fn preview_project_backup(&mut self, copying: bool) {
        self.backup = Backup {
            open: true,
            source: Some(Project {
                name: "Dreamcore".into(),
                path: "D:/Unreal Projects/Dreamcore/Dreamcore.uproject".into(),
                engine: "5.8".into(),
                description: String::new(),
                pinned: false,
                last_opened: 0,
                artwork: None,
            }),
            location: "E:/Project backups".into(),
            folder_name: "Dreamcore_Backup_2026-10-21_12-30-00".into(),
            ..Default::default()
        };
        if copying {
            self.backup.preview = true;
            self.backup.progress = Progress {
                phase: "Copying project files…".into(),
                file: "Content/Environments/City/City_Buildings.uasset".into(),
                copied: 3_200_000_000,
                total: 8_000_000_000,
                files_done: 320,
                files_total: 920,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_and_disconnection_release_backup_busy_state() {
        for disconnected in [false, true] {
            let (tx, rx) = mpsc::channel();
            let mut backup = Backup {
                rx: Some(rx),
                ..Default::default()
            };
            if !disconnected {
                tx.send(Event::Done(Ok("C:/Backups/Game".into()))).unwrap();
            }
            drop(tx);
            assert!(backup.busy());
            assert!(matches!(backup.next_event(), Some(Event::Done(_))));
            assert!(!backup.busy());
        }
    }
}
