use super::*;
use engine_workspace::{
    discovery,
    installer::{self, Control, Plan, Progress},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize},
    },
};
pub(super) enum Event {
    Prepared(Result<Plan, String>),
    Maintained(Installation, bool, Result<String, String>),
    Finished(Result<PathBuf, String>),
}
#[derive(Default)]
pub(super) struct Transfers {
    pub receiver: Option<Receiver<Event>>,
    pub plan: Option<Plan>,
    pub release: Option<Release>,
    pub parent: String,
    pub modal: bool,
    pub shortcut: bool,
    pub component_options: Option<super::engine_options::Options>,
    pub state: String,
    pub progress: Arc<Mutex<Progress>>,
    pub control: Control,
    pub downloading: bool,
    pub preparing: bool,
    pub installed: Option<PathBuf>,
    pub maintenance: Option<bool>,
}
impl Transfers {
    pub fn busy(&self) -> bool {
        self.receiver.is_some()
    }
    pub fn preparing(&self) -> bool {
        self.preparing
    }
    pub fn is_install_for(&self, app: &str) -> bool {
        self.maintenance.is_none()
            && self
                .release
                .as_ref()
                .is_some_and(|release| release.app == app)
    }
    pub fn dismiss_install(&mut self) -> bool {
        if self.busy() || self.maintenance.is_some() || self.installed.is_some() {
            return false;
        }
        // Only discard the UI plan. Verified staging files and cached chunks remain on disk.
        *self = Self::default();
        true
    }
    pub fn dismiss_install_for(&mut self, app: &str) -> bool {
        self.is_install_for(app) && self.dismiss_install()
    }
}
pub(super) enum ScanEvent {
    Found(Project),
    Finished(discovery::Report),
}

#[cfg(test)]
mod resume_tests {
    use super::*;

    fn stopped(target: PathBuf) -> Transfers {
        Transfers {
            release: Some(Release {
                app: "UE_5.8".into(),
                catalog_id: String::new(),
                channel: "Live".into(),
                platform: "Windows".into(),
                build: None,
                version: None,
            }),
            plan: Some(Plan {
                manifest: engine_workspace::buildpatch::Manifest::default(),
                bases: vec![],
                target,
                tags: vec![],
                selected: vec![],
            }),
            state: "Stopped. Verified files and chunks are retained.".into(),
            modal: true,
            ..Default::default()
        }
    }

    #[test]
    fn removing_matching_pending_engine_clears_resume_without_deleting_files() {
        let dir = tempfile::tempdir().unwrap();
        let chunk = dir.path().join("verified.chunk");
        std::fs::write(&chunk, b"verified data").unwrap();
        let mut transfer = stopped(dir.path().to_owned());
        assert!(!transfer.dismiss_install_for("UE_5.7"));
        assert!(transfer.plan.is_some());
        assert!(transfer.dismiss_install_for("UE_5.8"));
        assert!(transfer.plan.is_none());
        assert!(transfer.release.is_none());
        assert!(transfer.state.is_empty());
        assert!(!transfer.modal);
        assert_eq!(std::fs::read(chunk).unwrap(), b"verified data");
    }

    #[test]
    fn dismissal_preserves_active_transfers_and_completed_installs() {
        let mut transfer = stopped(PathBuf::from("unused"));
        let (_sender, receiver) = mpsc::channel();
        transfer.receiver = Some(receiver);
        assert!(!transfer.dismiss_install());
        assert!(transfer.busy());
        assert!(transfer.plan.is_some());
        assert!(
            !transfer
                .control
                .cancel
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        transfer.receiver = None;
        transfer.installed = Some(PathBuf::from("installed"));
        assert!(!transfer.dismiss_install());
        transfer.installed = None;
        transfer.maintenance = Some(false);
        assert!(!transfer.dismiss_install());
        transfer.maintenance = None;
        assert!(transfer.dismiss_install());
        assert!(transfer.plan.is_none());
    }
}
#[derive(Default)]
pub(super) struct Scan {
    pub receiver: Option<Receiver<ScanEvent>>,
    pub cancel: Arc<AtomicBool>,
    pub progress: Arc<AtomicUsize>,
    pub summary: String,
}
impl Workspace {
    pub(super) fn start_maintenance(&mut self, engine: Installation, remove: bool, ctx: Context) {
        if self.transfer.busy() || self.plugin_work.mutating {
            self.notify("Another engine operation is running.");
            return;
        }
        let control = Control::default();
        let progress = Arc::new(Mutex::new(Progress {
            phase: if remove {
                "Preparing removal".into()
            } else {
                "Loading the installed build manifest".into()
            },
            ..Default::default()
        }));
        let (tx, rx) = mpsc::channel();
        self.transfer = Transfers {
            receiver: Some(rx),
            control: control.clone(),
            progress: progress.clone(),
            downloading: true,
            maintenance: Some(remove),
            state: format!(
                "{} Unreal Engine {}",
                if remove { "Removing" } else { "Verifying" },
                engine.version
            ),
            ..Default::default()
        };
        self.plugin_engine = None;
        self.page = Page::Downloads;
        let snapshot = self.snapshot.clone();
        let session = storage::load_session();
        std::thread::spawn(move || {
            let result = if remove {
                engine_workspace::maintenance::remove(&engine, progress)
            } else {
                engine_workspace::maintenance::prepare(&engine, session.as_ref(), &snapshot)
                    .and_then(|plan| {
                        engine_workspace::maintenance::repair(&plan, &control, progress)
                    })
            };
            let _ = tx.send(Event::Maintained(engine, remove, result));
            ctx.request_repaint();
        });
    }

    pub(super) fn start_scan(&mut self, ctx: Context, roots: Option<Vec<PathBuf>>) {
        if self.scan.receiver.is_some() {
            return;
        }
        if discovery::prune_missing_projects(&mut self.projects) > 0 {
            self.persist();
        }
        let roots = roots.unwrap_or_else(|| {
            discovery::project_roots(&self.prefs.project_roots)
                .iter()
                .map(PathBuf::from)
                .collect()
        });
        if roots.is_empty() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        self.scan = Scan {
            receiver: Some(rx),
            cancel: cancel.clone(),
            progress: progress.clone(),
            summary: "Searching configured project folders…".into(),
        };
        std::thread::spawn(move || {
            let report = discovery::scan_stream(&roots, cancel, progress, |p| {
                let _ = tx.send(ScanEvent::Found(p));
                ctx.request_repaint();
            });
            let _ = tx.send(ScanEvent::Finished(report));
            ctx.request_repaint();
        });
    }
    pub(super) fn choose_install(&mut self, r: Release) {
        if self.transfer.busy() || self.plugin_work.mutating {
            self.page = Page::Downloads;
            return;
        }
        self.transfer.maintenance = None;
        self.transfer.installed = None;
        self.transfer.release = Some(r);
        self.transfer.parent = self.prefs.install_root.clone();
        self.transfer.plan = None;
        self.transfer.modal = true;
        self.transfer.shortcut = true;
        self.transfer.component_options = None;
        self.transfer.state.clear();
    }
    fn prepare_install(&mut self, ctx: Context) {
        if self.pending.is_some() || self.transfer.busy() {
            return;
        }
        let Some(s) = storage::load_session().or_else(|| self.session.clone()) else {
            self.login = true;
            return;
        };
        let Some(r) = self.transfer.release.clone() else {
            return;
        };
        let parent = PathBuf::from(&self.transfer.parent);
        let (tx, rx) = mpsc::channel();
        self.transfer.receiver = Some(rx);
        self.transfer.preparing = true;
        self.transfer.state = "Fetching Epic's engine manifest…".into();
        std::thread::spawn(move || {
            let result = installer::prepare(&s, &r, &parent);
            let _ = tx.send(Event::Prepared(result));
            ctx.request_repaint();
        });
    }
    fn run_install(&mut self, ctx: Context) {
        if self.transfer.busy() || self.plugin_work.mutating {
            return;
        }
        let Some(plan) = self.transfer.plan.clone() else {
            return;
        };
        let control = Control::default();
        let progress = Arc::new(Mutex::new(Progress::default()));
        let (tx, rx) = mpsc::channel();
        self.transfer.receiver = Some(rx);
        self.transfer.control = control.clone();
        self.transfer.progress = progress.clone();
        self.transfer.downloading = true;
        self.transfer.modal = false;
        self.transfer.state = "Preparing download…".into();
        self.page = Page::Downloads;
        std::thread::spawn(move || {
            let result = installer::install(&plan, &control, progress);
            let _ = tx.send(Event::Finished(result));
            ctx.request_repaint();
        });
    }
    pub(super) fn poll_transfers(&mut self, ctx: &Context) {
        let events: Vec<_> = self
            .scan
            .receiver
            .as_ref()
            .map(|r| r.try_iter().collect())
            .unwrap_or_default();
        let mut changed = false;
        for event in events {
            match event {
                ScanEvent::Found(project) => {
                    if self.demo {
                        continue;
                    }
                    changed |= discovery::merge(&mut self.projects, vec![project]) > 0;
                }
                ScanEvent::Finished(report) => {
                    self.scan.receiver = None;
                    self.scan.summary = format!(
                        "{}: {} projects; {} folders searched; {} inaccessible folders skipped.",
                        if report.cancelled {
                            "Scan stopped"
                        } else {
                            "Scan complete"
                        },
                        report.projects.len(),
                        report.directories,
                        report.skipped
                    );
                    self.notify(self.scan.summary.clone());
                }
            }
        }
        if changed {
            self.persist();
            self.load_assets(ctx);
        }
        if let Some(event) = self
            .transfer
            .receiver
            .as_ref()
            .and_then(|r| r.try_recv().ok())
        {
            self.transfer.receiver = None;
            self.transfer.preparing = false;
            self.transfer.downloading = false;
            self.session = storage::load_session();
            match &event {
                Event::Maintained(engine, removed, result) => self.record_activity(
                    if *removed {
                        "Engine removal".into()
                    } else {
                        if self.transfer.maintenance == Some(false) {
                            "Engine verification".into()
                        } else {
                            "Engine component changes".into()
                        }
                    },
                    engine.version.clone(),
                    result,
                ),
                Event::Finished(result) => {
                    let version = self
                        .transfer
                        .release
                        .as_ref()
                        .map(|r| {
                            r.version
                                .as_ref()
                                .map(ToString::to_string)
                                .unwrap_or_else(|| r.app.trim_start_matches("UE_").to_owned())
                        })
                        .unwrap_or_default();
                    self.record_activity(
                        "Engine installation".into(),
                        version,
                        &result
                            .as_ref()
                            .map(|p| format!("Installed {}", p.display()))
                            .map_err(Clone::clone),
                    );
                }
                _ => {}
            }
            match event {
                Event::Maintained(engine, removed, result) => match result {
                    Ok(message) => {
                        if removed {
                            self.prefs.engine_paths.retain(|p| p != &engine.path);
                            if self.prefs.default_engine == engine.path {
                                self.prefs.default_engine.clear();
                            }
                            if self.launch_path.as_ref() == Some(&engine.path) {
                                self.launch_path = None;
                            }
                            self.plugin_counts.remove(&engine.path);
                            self.workspace_cache.invalidate();
                        }
                        self.persist();
                        self.engines = platform::display_engines();
                        self.plugin_engine = None;
                        self.transfer.state = message.clone();
                        self.notify(message);
                    }
                    Err(message) => {
                        self.transfer.state = message.clone();
                        self.notify(message);
                    }
                },
                Event::Prepared(Ok(mut p)) => {
                    p.target = PathBuf::from(&self.transfer.parent).join(&p.manifest.app);
                    self.transfer.state =
                        "Manifest verified. Choose components and installation destination.".into();
                    self.transfer.plan = Some(p);
                }
                Event::Prepared(Err(e)) => self.transfer.state = e,
                Event::Finished(Ok(path)) => {
                    if !self
                        .prefs
                        .engine_paths
                        .contains(&path.to_string_lossy().to_string())
                    {
                        self.prefs.engine_paths.push(path.to_string_lossy().into());
                    }
                    if let Some(release) = &self.transfer.release {
                        self.prefs.pending_engines.retain(|app| app != &release.app);
                    }
                    self.persist();
                    self.engines = platform::display_engines();
                    if self.transfer.shortcut {
                        if let Some(engine) = platform::inspect_engine(&path, "") {
                            if let Err(e) = platform::create_engine_shortcut(&engine) {
                                self.notify(format!("Engine installed; shortcut failed: {e}"));
                            }
                        }
                    }
                    let registration: Option<serde_json::Value> =
                        storage::read_json(&path.join(".alpine-epic-registration.json"));
                    self.transfer.state = registration.as_ref().and_then(|v|v["warning"].as_str()).map(|w|format!("Engine installed and verified; Epic registration needs attention: {w}")).unwrap_or_else(||"Engine installed, verified and registered with Epic. Ready to launch.".into());
                    self.transfer.installed = Some(path);
                    self.notify(self.transfer.state.clone());
                    if self.prefs.downloads.notify_complete {
                        ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(
                            egui::UserAttentionType::Informational,
                        ));
                    }
                }
                Event::Finished(Err(e)) => self.transfer.state = e,
            }
        }
    }
    pub(super) fn pending_engine_card(
        &mut self,
        ui: &mut Ui,
        c: &Canvas,
        index: usize,
        r: [f32; 4],
    ) {
        let [x, y, w, h] = r;
        let (pending_fill, pending_border) = if self.theme.dark {
            ([27, 31, 34], [80, 90, 100])
        } else {
            ([226, 232, 241], [173, 185, 201])
        };
        c.fill(ui, r, pending_fill, 6.);
        // Rounded dashed outline distinguishes a planned installation from an installed engine.
        let bounds = c.rect(r).shrink(0.75 * c.scale());
        let radius = 6. * c.scale();
        let mut outline = Vec::new();
        for (center, start) in [
            (pos2(bounds.right() - radius, bounds.top() + radius), -90.),
            (pos2(bounds.right() - radius, bounds.bottom() - radius), 0.),
            (pos2(bounds.left() + radius, bounds.bottom() - radius), 90.),
            (pos2(bounds.left() + radius, bounds.top() + radius), 180.),
        ] {
            for step in 0..=8 {
                let angle = (start + step as f32 * 90. / 8.).to_radians();
                outline.push(center + vec2(angle.cos(), angle.sin()) * radius);
            }
        }
        outline.push(outline[0]);
        ui.painter().extend(egui::Shape::dashed_line(
            &outline,
            Stroke::new(c.scale(), color(pending_border)),
            5. * c.scale(),
            3. * c.scale(),
        ));
        let releases: Vec<_> = self
            .snapshot
            .releases
            .iter()
            .filter(|r| r.platform == "Windows" && r.version.is_some())
            .cloned()
            .collect();
        let current = self.prefs.pending_engines[index].clone();
        let selected = releases.iter().find(|r| r.app == current);
        let title = selected
            .and_then(|r| r.version.as_ref())
            .map(ToString::to_string)
            .unwrap_or_else(|| "Choose version".into());
        let mut choice = current.clone();
        ui.scope_builder(
            UiBuilder::new().max_rect(c.rect([x + 20., y + 25., w - 40., 38.])),
            |ui| {
                ui.visuals_mut().widgets.inactive.bg_fill = color(self.theme.background);
                ui.visuals_mut().widgets.inactive.weak_bg_fill = color(self.theme.background);
                ComboBox::from_id_salt(("pending-engine-version", index))
                    .selected_text(title)
                    .width(w - 54.)
                    .show_ui(ui, |ui| {
                        for release in &releases {
                            ui.selectable_value(
                                &mut choice,
                                release.app.clone(),
                                release.version.as_ref().unwrap().to_string(),
                            );
                        }
                    });
            },
        );
        if choice != current {
            self.prefs.pending_engines[index] = choice;
            self.persist();
        }
        c.circle(ui, x + 24., y + 100., 6., [235, 150, 25]);
        c.text(
            ui,
            x + 40.,
            y + 100.,
            "Not installed",
            14.,
            false,
            self.theme.muted,
            Align2::LEFT_CENTER,
        );
        let menu = ui
            .interact(
                c.rect([x + w - 42., y + h - 43., 28., 32.]),
                Id::new(("pending-engine-menu", index)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        for offset in [-5., 0., 5.] {
            c.circle(ui, x + w - 28., y + h - 27. + offset, 1.7, self.theme.muted);
        }
        let mut remove = false;
        let active_install = self.transfer.busy() && self.transfer.is_install_for(&current);
        Popup::menu(&menu)
            .frame(menu_frame(&self.theme))
            .show(|ui| {
                style_menu(ui, &self.theme);
                if ui
                    .add_enabled(!active_install, Button::new("Remove"))
                    .on_disabled_hover_text(
                        "Stop this engine installation in Downloads before removing it.",
                    )
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    remove = true;
                    ui.close();
                }
            });
        if remove {
            self.prefs.pending_engines.remove(index);
            self.transfer.dismiss_install_for(&current);
            self.persist();
            return;
        }
        let selected = releases
            .iter()
            .find(|r| r.app == self.prefs.pending_engines[index])
            .cloned();
        if c.button(
            ui,
            ("pending-engine-install", index),
            button_slot([x + 14., y + h - 43., w - 66., 32.]),
            "Install",
            None,
            true,
        ) {
            if self.transfer.busy() || self.plugin_work.mutating {
                self.notify("Another installation is running.");
                return;
            }
            if let Some(release) = selected {
                self.choose_install(release);
                if self.transfer.modal {
                    self.prepare_install(ui.ctx().clone());
                }
            } else {
                self.notify(if releases.is_empty() {
                    "Check for engine releases first."
                } else {
                    "Choose an engine version first."
                });
            }
        }
    }
    pub(super) fn install_dialog(&mut self, ctx: &Context) {
        if !self.transfer.modal {
            return;
        }
        if self.transfer.component_options.is_some() {
            self.install_component_dialog(ctx);
            return;
        }
        let mut open = true;
        let mut prepare = false;
        let mut install = false;
        let mut options = false;
        dialog::show(
            ctx,
            &self.theme.clone(),
            "install-engine",
            "Choose install location",
            "Choose a folder and installation options for Unreal Engine.",
            &mut open,
            |ui| {
                let app = self
                    .transfer
                    .release
                    .as_ref()
                    .map(|r| r.app.clone())
                    .unwrap_or_default();
                ui.label("Folder");
                dialog::folder(ui, &mut self.transfer.parent);
                let target = PathBuf::from(&self.transfer.parent).join(&app);
                let occupied = target.exists()
                    && (!target.is_dir()
                        || std::fs::read_dir(&target)
                            .map(|mut d| d.next().is_some())
                            .unwrap_or(true));
                dialog::info(ui, &self.theme, "Path", &target.to_string_lossy());
                if occupied {
                    ui.colored_label(
                        color(self.theme.warning),
                        "Directory must be empty. Choose another folder.",
                    );
                }
                if let Some(plan) = &mut self.transfer.plan {
                    plan.target = target;
                    options = ui
                        .button("Options")
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked();
                    let (disk, download, _) = plan.sizes();
                    ui.label(format!(
                        "Download size: {:.2} GB    Required storage: {:.2} GB",
                        download as f64 / 1e9,
                        disk as f64 / 1e9
                    ));
                } else if self.transfer.preparing {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading installation options…");
                    });
                } else {
                    prepare = ui
                        .button(if self.session.is_some() {
                            "Load installation options"
                        } else {
                            "Sign in to Epic"
                        })
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked();
                }
                ui.checkbox(&mut self.transfer.shortcut, "Create desktop shortcut");
                if !self.transfer.state.is_empty() {
                    ui.label(&self.transfer.state);
                }
                install = dialog::footer(
                    ui,
                    &self.theme,
                    "Install",
                    self.transfer.plan.is_some()
                        && !self.transfer.busy()
                        && !occupied
                        && Path::new(&self.transfer.parent).is_absolute()
                        && !app.is_empty(),
                    false,
                );
            },
        );
        self.transfer.modal &= open;
        if prepare {
            self.prepare_install(ctx.clone());
        }
        if options {
            if let Some(plan) = &self.transfer.plan {
                self.transfer.component_options = Some(
                    super::engine_options::Options::from_install_plan(plan.clone()),
                );
            }
        }
        if install {
            self.prefs.install_root = self.transfer.parent.clone();
            self.persist();
            self.run_install(ctx.clone());
        }
    }
}
