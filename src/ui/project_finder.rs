use super::*;
use engine_workspace::discovery::{self, suggestions::Suggestion};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[derive(Default)]
pub(super) struct Finder {
    pub open: bool,
    rx: Option<Receiver<FinderEvent>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
    rows: Vec<(bool, Suggestion)>,
    summary: String,
    found: usize,
    full_scan: bool,
}
enum FinderEvent {
    Results(Vec<Suggestion>, usize),
    Finished(discovery::Report, Vec<Suggestion>),
}
impl Finder {
    // Render-only fixture; never scans drives or saves project folders.
    pub(super) fn preview(&mut self) {
        self.open = true;
        self.summary =
            "Scan complete: 12 projects in 6 suggested folders. Review the folders to add.".into();
        self.rows = (0..6)
            .map(|i| {
                (
                    true,
                    Suggestion {
                        path: PathBuf::from(format!("D:/Unreal Projects/Studio {i}/Projects")),
                        projects: (0..2)
                            .map(|n| Project {
                                name: format!("Project {n}"),
                                path: String::new(),
                                description: String::new(),
                                engine: "5.8".into(),
                                pinned: false,
                                last_opened: 0,
                                artwork: None,
                            })
                            .collect(),
                    },
                )
            })
            .collect();
    }
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
}
impl Drop for Finder {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Workspace {
    pub(super) fn open_project_finder(&mut self) {
        self.project_finder.open = true;
    }
    fn find_project_folders(&mut self, ctx: Context, full_scan: bool) {
        if self.project_finder.busy() || self.demo {
            return;
        }
        let configured = discovery::project_roots(&self.prefs.project_roots);
        let excluded: Vec<_> = configured
            .iter()
            .chain(self.engines.iter().map(|e| &e.path))
            .map(PathBuf::from)
            .collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        self.project_finder = Finder {
            open: true,
            rx: Some(rx),
            cancel: cancel.clone(),
            progress: progress.clone(),
            rows: vec![],
            summary: String::new(),
            found: 0,
            full_scan,
        };
        std::thread::spawn(move || {
            let roots = discovery::quick::preferred_roots();
            let mut projects = vec![];
            let mut published = Instant::now();
            let found = |project| {
                projects.push(project);
                if projects.len() == 1 || published.elapsed() >= Duration::from_millis(200) {
                    let rows = discovery::suggestions::folders(projects.clone(), &configured);
                    let _ = tx.send(FinderEvent::Results(rows, projects.len()));
                    published = Instant::now();
                    ctx.request_repaint();
                }
            };
            let report = if full_scan {
                discovery::scan_excluding(&roots, &excluded, cancel, progress, found)
            } else {
                discovery::scan_quick(&roots, &excluded, cancel, progress, found)
            };
            let rows = discovery::suggestions::folders(report.projects.clone(), &configured);
            let _ = tx.send(FinderEvent::Finished(report, rows));
            ctx.request_repaint();
        });
    }
    pub(super) fn poll_project_finder(&mut self) {
        for _ in 0..100 {
            let Some(rx) = &self.project_finder.rx else {
                break;
            };
            match rx.try_recv() {
                Ok(FinderEvent::Results(rows, count)) => {
                    self.project_finder.found = count;
                    self.project_finder.rows = rows.into_iter().map(|s| (true, s)).collect();
                }
                Ok(FinderEvent::Finished(report, rows)) => {
                    self.project_finder.rx = None;
                    self.project_finder.found = report.projects.len();
                    self.project_finder.rows = rows.into_iter().map(|s| (true, s)).collect();
                    self.project_finder.summary = format!(
                        "{}: {} projects found. {} folders searched; {} inaccessible or linked folders skipped.{}",
                        if report.cancelled {
                            "Scan stopped"
                        } else if self.project_finder.full_scan {
                            "Full scan complete"
                        } else {
                            "Quick search finished"
                        },
                        self.project_finder.found,
                        report.directories,
                        report.skipped,
                        if !self.project_finder.full_scan {
                            " Missing a project? Try Full scan or choose its folder."
                        } else {
                            ""
                        }
                    );
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.project_finder.rx = None;
                    self.project_finder.summary =
                        "The scan ended unexpectedly. Partial results are available; you can retry.".into();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
    }
    pub(super) fn project_finder_dialog(&mut self, ctx: &Context) {
        if !self.project_finder.open {
            return;
        }
        let mut open = true;
        let theme = self.theme.clone();
        let mut scan = false;
        let mut full_scan = false;
        let mut choose = false;
        let mut add = false;
        dialog::show(
            ctx,
            &theme,
            "project-finder",
            "Find your Unreal projects",
            "Find projects quickly, then review the folders to add.",
            &mut open,
            |ui| {
                ui.label("Choose a folder containing one or many projects. Subfolders are included. Added folders are saved in Settings → Project folders.");
                ui.label(RichText::new("Quick search checks Unreal/Epic history and nearby folders first.").color(color(theme.muted)))
                    .on_hover_text("Then searches local drives up to 4 folders deep, stopping after 3,000 folders or about 8 seconds. Full scan searches deeper. For a network drive, choose its folder. Configured folders, engines and generated folders are skipped.");
                ui.horizontal_wrapped(|ui| {
                    scan = ui
                        .add_enabled(
                            !self.project_finder.busy() && !self.demo,
                            primary_button("Quick search", &theme),
                        )
                        .clicked();
                    full_scan = ui.add_enabled(
                        !self.project_finder.busy() && !self.demo,
                        buttons::secondary_button("Full scan"),
                    ).on_hover_text("Search all local drives, including deeply nested folders. This can take a while on large drives.").clicked();
                    choose = ui
                        .add_enabled(
                            !self.project_finder.busy(),
                            buttons::secondary_button("Choose a folder"),
                        )
                        .on_hover_text("Select a single project's folder or a parent folder containing many projects. Subfolders are searched automatically.")
                        .clicked();
                    if self.project_finder.busy() {
                        if ui.add(buttons::secondary_button("Stop scan")).clicked() {
                            self.project_finder.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                });
                if self.project_finder.busy() {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().color(color(theme.accent)));
                        ui.label(format!(
                            "{} · {} folders searched · {} projects found…",
                            if self.project_finder.full_scan {
                                "Full scan"
                            } else {
                                "Quick search"
                            },
                            self.project_finder.progress.load(Ordering::Relaxed),
                            self.project_finder.found
                        ));
                    });
                    ctx.request_repaint_after(Duration::from_millis(200));
                } else if !self.project_finder.summary.is_empty() {
                    ui.label(&self.project_finder.summary);
                    if self.project_finder.rows.is_empty() {
                        ui.label("No new project folders found. Already configured folders are skipped; you can choose a custom folder directly.");
                    }
                }
                if !self.project_finder.rows.is_empty() {
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.project_finder.busy(),
                                buttons::secondary_button("Select all"),
                            )
                            .clicked()
                        {
                            self.project_finder.rows.iter_mut().for_each(|r| r.0 = true);
                        }
                        if ui
                            .add_enabled(
                                !self.project_finder.busy(),
                                buttons::secondary_button("Clear selection"),
                            )
                            .clicked()
                        {
                            self.project_finder
                                .rows
                                .iter_mut()
                                .for_each(|r| r.0 = false);
                        }
                        let count = self.project_finder.rows.iter().filter(|r| r.0).count();
                        add = ui
                            .add_enabled(
                                count > 0 && !self.project_finder.busy(),
                                primary_button(format!("Add selected folders ({count})"), &theme),
                            )
                            .clicked();
                    });
                    let scanning = self.project_finder.busy();
                    AppScrollArea::vertical(&theme)
                        .id_salt("project-folder-suggestions")
                        .max_height((ctx.content_rect().height() - 520.).clamp(80., 300.))
                        .show(ui, |ui| {
                            for (selected, suggestion) in &mut self.project_finder.rows {
                                let path = dialog::display_path(&suggestion.path.to_string_lossy());
                                ui.horizontal(|ui| {
                                    ui.add_enabled(
                                        !scanning,
                                        buttons::checkbox(selected, "", &self.theme),
                                    );
                                    ui.vertical(|ui| {
                                        ui.add(Label::new(&path).truncate()).on_hover_text(&path);
                                        ui.label(
                                            RichText::new(format!(
                                                "{} project{}",
                                                suggestion.projects.len(),
                                                if suggestion.projects.len() == 1 {
                                                    ""
                                                } else {
                                                    "s"
                                                }
                                            ))
                                            .color(color(theme.muted)),
                                        )
                                        .on_hover_text(
                                            suggestion
                                                .projects
                                                .iter()
                                                .map(|p| p.name.as_str())
                                                .collect::<Vec<_>>()
                                                .join("\n"),
                                        );
                                    });
                                });
                                ui.separator();
                            }
                        });
                }
            },
        );
        self.project_finder.open = open;
        if !open {
            self.project_finder.cancel.store(true, Ordering::Relaxed);
        }
        if scan || full_scan {
            self.find_project_folders(ctx.clone(), full_scan);
        }
        if choose {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Choose a folder containing one or more Unreal projects")
                .pick_folder()
            {
                self.add_project_root(path, ctx);
                self.project_finder.open = false;
                self.show_discovered_projects();
            }
        }
        if add {
            let paths = self
                .project_finder
                .rows
                .iter()
                .filter(|r| r.0)
                .map(|r| r.1.path.clone())
                .collect();
            self.add_project_roots(paths, ctx);
            self.project_finder.open = false;
            self.project_finder.rows.clear();
            self.show_discovered_projects();
        }
    }
    fn show_discovered_projects(&mut self) {
        self.page = Page::Projects;
        self.search.clear();
        self.filter = "All versions".into();
        self.category_filter = None;
        self.pinned_only = false;
        self.project_scroll = 0.;
    }
}
