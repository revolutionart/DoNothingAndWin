use super::*;
pub(super) use engine_workspace::engine_operation::Event;
use engine_workspace::{
    discovery,
    engine_operation::{Kind, Operation},
    installer,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
};
#[derive(Default)]
pub(super) struct Transfers {
    pub operation: Operation,
    pub release: Option<Release>,
    pub parent: String,
    pub modal: bool,
    pub shortcut: bool,
    pub component_options: Option<super::engine_options::Options>,
    pub state: String,
    pub activity: super::plugin_activity::Activity,
}
impl Transfers {
    pub fn busy(&self) -> bool {
        self.operation.busy()
    }
    pub fn preparing(&self) -> bool {
        self.operation.preparing()
    }
    pub fn is_install_for(&self, app: &str) -> bool {
        self.operation.kind() == Kind::Install
            && self
                .release
                .as_ref()
                .is_some_and(|release| release.app == app)
    }
    pub fn dismiss_install(&mut self) -> bool {
        if !self.operation.dismiss() {
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

pub(super) fn release_is_installed(release: &Release, engines: &[Installation]) -> bool {
    let version = release
        .version
        .clone()
        .or_else(|| release.build.as_deref().and_then(EngineVersion::parse));
    engines.iter().any(|engine| {
        let Some(installed) = EngineVersion::parse(&engine.version) else {
            return false;
        };
        if let Some(version) = &version {
            installed == *version
        } else {
            EngineVersion::parse(&release.app).is_some_and(|branch| {
                installed.major == branch.major && installed.minor == branch.minor
            })
        }
    })
}

#[cfg(test)]
mod resume_tests {
    use super::*;

    #[test]
    fn installed_versions_are_excluded_but_new_hotfixes_remain_available() {
        let engines = vec![Installation {
            version: "5.8.3".into(),
            path: "installed-engine".into(),
            association: "5.8".into(),
        }];
        let mut release = Release {
            app: "UE_5.8".into(),
            catalog_id: String::new(),
            channel: "Live".into(),
            platform: "Windows".into(),
            build: None,
            version: EngineVersion::parse("5.8.3"),
        };
        assert!(release_is_installed(&release, &engines));
        assert!(!release_is_installed(&release, &[]));
        release.version = EngineVersion::parse("5.8.4");
        assert!(!release_is_installed(&release, &engines));
        release.version = EngineVersion::parse("5.7.4");
        assert!(!release_is_installed(&release, &engines));
        release.version = None;
        release.build = Some("5.8.3-58210709+++UE5+Release-5.8-Windows".into());
        assert!(release_is_installed(&release, &engines));
        release.build = None;
        assert!(release_is_installed(&release, &engines));
        release.app = "UE_5.7".into();
        assert!(!release_is_installed(&release, &engines));
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
    pub(super) fn start_engine_update(
        &mut self,
        engine: Installation,
        release: Release,
        ctx: Context,
    ) {
        if self.transfer.busy()
            || self.plugin_work.mutating
            || self.library_work_pending()
            || self.launcher_updates.blocks_work()
        {
            self.page = Page::Downloads;
            self.notify("Finish or stop the current installation before updating an engine.");
            return;
        }
        let Some(session) = storage::load_session().or_else(|| self.session.clone()) else {
            self.login = true;
            self.notify("Sign in to Epic, then click Update to update this engine.");
            return;
        };
        self.transfer = Transfers {
            state: format!(
                "Updating Unreal Engine {} to {}",
                engine.version,
                release
                    .version
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default()
            ),
            release: Some(release.clone()),
            ..Default::default()
        };
        let worker = self
            .transfer
            .operation
            .maintain(Kind::Update, engine.clone())
            .unwrap();
        self.page = Page::Downloads;
        self.plugin_engine = None;
        let snapshot = self.snapshot.clone();
        std::thread::spawn(move || {
            let result = engine_workspace::engine_update::run(
                &engine,
                &release,
                &session,
                &snapshot,
                &worker.control,
                worker.progress,
            );
            let _ = worker.events.send(Event::Maintained(engine, false, result));
            ctx.request_repaint();
        });
    }

    pub(super) fn start_maintenance(&mut self, engine: Installation, remove: bool, ctx: Context) {
        if self.transfer.busy() || self.plugin_work.mutating {
            self.notify("Another engine operation is running.");
            return;
        }
        self.transfer = Transfers {
            state: format!(
                "{} Unreal Engine {}",
                if remove { "Removing" } else { "Verifying" },
                engine.version
            ),
            ..Default::default()
        };
        let worker = self
            .transfer
            .operation
            .maintain(
                if remove { Kind::Remove } else { Kind::Verify },
                engine.clone(),
            )
            .unwrap();
        let (tx, control, progress) = (worker.events, worker.control, worker.progress);
        progress.lock().unwrap().phase = if remove {
            "Preparing removal"
        } else {
            "Loading the installed build manifest"
        }
        .into();
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
                crate::window_frame::progress_repaint(&ctx);
            });
            let _ = tx.send(ScanEvent::Finished(report));
            ctx.request_repaint();
        });
    }
    pub(super) fn choose_install(&mut self, r: Release, ctx: Context) {
        if release_is_installed(&r, &self.engines) {
            self.transfer.modal = false;
            self.notify("This engine version is already installed.");
            return;
        }
        if self.transfer.busy() || self.plugin_work.mutating {
            self.page = Page::Downloads;
            return;
        }
        self.transfer.operation = Operation::default();
        self.transfer.release = Some(r);
        self.transfer.parent = self.prefs.install_root.clone();
        self.transfer.modal = true;
        self.transfer.shortcut = true;
        self.transfer.component_options = None;
        self.transfer.state.clear();
        self.prepare_install(ctx);
    }
    fn prepare_install(&mut self, ctx: Context) {
        if self.transfer.busy() {
            return;
        }
        let Some(s) = storage::load_session().or_else(|| self.session.clone()) else {
            self.login = true;
            return;
        };
        let Some(r) = self.transfer.release.clone() else {
            return;
        };
        if release_is_installed(&r, &self.engines) {
            self.transfer.modal = false;
            self.notify("This engine version is already installed.");
            return;
        }
        let parent = PathBuf::from(&self.transfer.parent);
        let Some(worker) = self.transfer.operation.prepare(parent.clone()) else {
            return;
        };
        let tx = worker.events;
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
        let Some(plan) = self.transfer.operation.plan().cloned() else {
            return;
        };
        // Recheck local installations before starting a previously prepared plan.
        if !self.demo {
            self.engines = platform::display_engines();
        }
        if self
            .transfer
            .release
            .as_ref()
            .is_some_and(|r| release_is_installed(r, &self.engines))
        {
            self.transfer.modal = false;
            self.notify("This engine version is already installed.");
            return;
        }
        let Some(worker) = self.transfer.operation.install() else {
            return;
        };
        let (tx, control, progress) = (worker.events, worker.control, worker.progress);
        self.transfer.activity = Default::default();
        self.transfer
            .activity
            .sample_totals("engine", [0; 3], Instant::now());
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
        if self.transfer.operation.transferring() && self.transfer.operation.kind() == Kind::Install
        {
            let p = self.transfer.operation.progress();
            self.transfer.activity.sample_totals(
                "engine",
                [p.network, p.read, p.written],
                Instant::now(),
            );
            ctx.request_repaint_after(Duration::from_millis(150));
        }
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
        if let Some(event) = self.transfer.operation.poll() {
            self.session = storage::load_session();
            match &event {
                Event::Maintained(engine, removed, result) => self.record_activity(
                    if *removed {
                        "Engine removal".into()
                    } else {
                        if self.transfer.operation.kind() == Kind::Update {
                            "Engine update".into()
                        } else if self.transfer.operation.kind() == Kind::Verify {
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
                        self.workspace_cache.invalidate();
                        self.plugin_engine = None;
                        self.transfer.state = message.clone();
                        self.notify(message);
                    }
                    Err(message) => {
                        self.transfer.state = message.clone();
                        self.notify(message);
                    }
                },
                Event::Prepared(Ok(_)) => {
                    self.transfer.state =
                        "Manifest verified. Choose components and installation destination.".into();
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
            .filter(|r| {
                r.platform == "Windows"
                    && r.version.is_some()
                    && !release_is_installed(r, &self.engines)
            })
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
            UiBuilder::new().max_rect(c.rect([x + 20., y + 15., w - 40., 32.])),
            |ui| {
                ui.visuals_mut().widgets.inactive.bg_fill = color(self.theme.background);
                ui.visuals_mut().widgets.inactive.weak_bg_fill = color(self.theme.background);
                AppComboBox::new(&self.theme, ("pending-engine-version", index))
                    .selected_text(title)
                    .width(w - 54.)
                    .show_ui(ui, |ui| {
                        if releases.is_empty() {
                            ui.label("No engine versions available to install.");
                        }
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
        c.circle(ui, x + 24., y + 69., 6., [235, 150, 25]);
        c.text(
            ui,
            x + 40.,
            y + 69.,
            "Not installed",
            14.,
            false,
            self.theme.muted,
            Align2::LEFT_CENTER,
        );
        let menu = ui
            .interact(
                c.rect([x + w - 42., y + h - 46., 28., 32.]),
                Id::new(("pending-engine-menu", index)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        for offset in [-5., 0., 5.] {
            c.circle(ui, x + w - 28., y + h - 30. + offset, 1.7, self.theme.muted);
        }
        let mut remove = false;
        let active_install = self.transfer.busy() && self.transfer.is_install_for(&current);
        Popup::menu(&menu)
            .frame(menu_frame(&self.theme))
            .show(|ui| {
                style_menu(ui, &self.theme);
                if ui
                    .add_enabled(!active_install, buttons::secondary_button("Remove"))
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
            button_slot([x + 14., y + h - 46., w - 66., COMPACT_BUTTON_HEIGHT]),
            "Install",
            None,
            true,
        ) {
            if self.transfer.busy() || self.plugin_work.mutating {
                self.notify("Another installation is running.");
                return;
            }
            if let Some(release) = selected {
                self.choose_install(release, ui.ctx().clone());
            } else {
                self.notify(if releases.is_empty() {
                    if self.snapshot.releases.is_empty() {
                        "Check for engine releases first."
                    } else {
                        "No engine versions available to install."
                    }
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
                if let Some(plan) = self.transfer.operation.plan_mut() {
                    plan.target = target;
                    options = ui
                        .add(buttons::secondary_button("Options"))
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked();
                    let (disk, download, _) = plan.sizes();
                    ui.label(format!(
                        "Download size: {:.2} GB    Required storage: {:.2} GB",
                        download as f64 / 1e9,
                        disk as f64 / 1e9
                    ));
                } else if self.transfer.preparing() {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().color(color(self.theme.text)));
                        ui.label("Loading installation options…");
                    });
                } else {
                    prepare = ui
                        .add(buttons::secondary_button(if self.session.is_some() {
                            "Load installation options"
                        } else {
                            "Sign in to Epic"
                        }))
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked();
                }
                ui.add(buttons::checkbox(
                    &mut self.transfer.shortcut,
                    "Create desktop shortcut",
                    &self.theme,
                ));
                if !self.transfer.state.is_empty() {
                    ui.label(&self.transfer.state);
                }
                install = dialog::footer(
                    ui,
                    &self.theme,
                    "Install",
                    self.transfer.operation.plan().is_some()
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
            if let Some(plan) = self.transfer.operation.plan() {
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
