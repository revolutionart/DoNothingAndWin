use super::*;
use engine_workspace::discovery;
#[derive(Default)]
pub(super) struct EngineSpace {
    key: Vec<String>,
    result: Option<(u64, bool)>,
    rx: Option<Receiver<(u64, bool)>>,
    checked: Option<Instant>,
}
impl Workspace {
    pub(super) fn add_project_root(&mut self, path: std::path::PathBuf, ctx: &Context) {
        self.add_project_roots(vec![path], ctx);
    }
    pub(super) fn add_project_roots(&mut self, paths: Vec<std::path::PathBuf>, ctx: &Context) {
        let mut added = 0;
        for path in paths {
            if !path.is_dir() {
                continue;
            }
            if discovery::project_roots(&self.prefs.project_roots)
                .iter()
                .any(|root| discovery::inside(&path, Path::new(root)))
            {
                continue;
            }
            self.prefs
                .project_roots
                .push(dialog::display_path(&path.to_string_lossy()));
            added += 1;
        }
        if added == 0 {
            self.notify("These folders are already tracked or are no longer accessible.");
            return;
        }
        self.reconcile_project_roots(ctx);
        self.notify(format!(
            "Added {added} project folders to Settings. Finding their projects…"
        ));
    }
    fn reconcile_project_roots(&mut self, ctx: &Context) {
        self.scan
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.scan = transfers::Scan::default();
        discovery::retain_project_roots(
            &mut self.projects,
            &discovery::project_roots(&self.prefs.project_roots),
            &self.prefs.manual_projects,
        );
        self.persist();
        self.start_scan(ctx.clone(), None);
    }
    pub(super) fn engine_storage_badge(&mut self, ui: &mut Ui, c: &Canvas) {
        let mut key: Vec<_> = self
            .engines
            .iter()
            .filter(|e| self.workspace_cache.engines.get(&e.path) == Some(&true))
            .map(|e| e.path.clone())
            .collect();
        key.sort();
        key.dedup();
        if self.engine_space.rx.is_none()
            && (self.engine_space.key != key
                || self
                    .engine_space
                    .checked
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(300)))
        {
            self.engine_space.key = key.clone();
            self.engine_space.result = None;
            self.engine_space.checked = Some(Instant::now());
            let (tx, rx) = mpsc::channel();
            self.engine_space.rx = Some(rx);
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let paths = key.iter().map(std::path::PathBuf::from).collect::<Vec<_>>();
                let _ = tx.send(discovery::installed_bytes(&paths));
                ctx.request_repaint();
            });
        }
        if let Some(result) = self
            .engine_space
            .rx
            .as_ref()
            .and_then(|r| r.try_recv().ok())
        {
            self.engine_space.result = Some(result);
            self.engine_space.rx = None;
        }
        let r = c.rect([510., 130., 180., 28.]);
        let p = ui.painter();
        let tint = color(self.theme.muted);
        let s = Stroke::new(1.5, tint);
        let o = r.min + vec2(2., 5.);
        p.add(Shape::line(
            vec![
                o + vec2(3., 15.),
                o + vec2(0., 12.),
                o + vec2(0., 8.),
                o + vec2(3., 5.),
                o + vec2(7., 5.),
                o + vec2(9., 1.),
                o + vec2(14., 0.),
                o + vec2(19., 3.),
                o + vec2(20., 7.),
                o + vec2(24., 8.),
                o + vec2(26., 11.),
                o + vec2(24., 15.),
                o + vec2(3., 15.),
            ],
            s,
        ));
        let label = self
            .engine_space
            .result
            .map(|(n, complete)| {
                format!(
                    "{}{:.1} GB",
                    if complete { "" } else { "≥ " },
                    n as f64 / 1_000_000_000.
                )
            })
            .unwrap_or_else(|| "Calculating…".into());
        p.text(
            r.min + vec2(36., 14.),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.),
            tint,
        );
        ui.interact(r, Id::new("installed-engine-size"), Sense::hover())
            .on_hover_text("Total size of all installed engines");
    }
    pub(super) fn settings_flow(&mut self, ui: &mut Ui, c: &Canvas, ctx: &Context) {
        let top = PageHeaderLayout::new().divider_y + 14.;
        let area = c.rect([271., top, c.width - 297., c.height - top - 62.]);
        let theme = self.theme.clone();
        let mut remove = None;
        ui.scope_builder(UiBuilder::new().max_rect(area),|ui|{
            ui.style_mut().text_styles.insert(TextStyle::Body,FontId::proportional(16.));
            ui.style_mut().text_styles.insert(TextStyle::Button,FontId::proportional(COMPACT_BUTTON_FONT_SIZE));
            ui.style_mut().text_styles.insert(TextStyle::Heading,FontId::proportional(23.));
            AppScrollArea::vertical(&theme).inner_margin(16.).id_salt("settings-flow").auto_shrink([false,true]).show(ui,|ui|{
                ui.set_width(ui.available_width());ui.spacing_mut().item_spacing=vec2(14.,14.);ui.spacing_mut().button_padding=vec2(18.,10.);
                settings_section(ui,&theme,|ui|{
                ui.heading("Epic account");
                ui.horizontal_wrapped(|ui|{
                    ui.label(self.session.as_ref().map(|s|s.display_name.as_str()).unwrap_or("Not connected"));
                    if self.session.is_some(){if ui.add(buttons::secondary_button("Sign out")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.sign_out_account();}}else if ui.add(buttons::secondary_button("Sign in with Epic")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.login=true;}
                });
                ui.label("Each developer signs in with their own account. Sessions are encrypted for this Windows user.");
                });
                settings_section(ui,&theme,|ui|{
                ui.heading("Project folders");
                if downloads::toggle(ui, &theme, &mut self.prefs.scan_projects_on_startup, "Scan project folders on startup", "Search configured project folders when the app starts. Turn off to use Rescan folders or the project refresh button manually.") { self.persist(); }
                if downloads::toggle(ui, &theme, &mut self.prefs.notify_project_scan, "Show scan-completion notifications", "Show the scan result popup. Results and skipped folder paths remain available in Downloads when this is off.") { self.persist(); }
                if !self.scan.summary.is_empty() && ui.add(buttons::secondary_button("View latest scan results")).clicked() {
                    self.page = Page::Downloads;
                    self.downloads.settings = false;
                    self.downloads.show_overview();
                }
                ui.label("Add a single project's folder or a parent folder containing many projects. Projects in all subfolders appear automatically. Removing a folder only removes its projects from this launcher; files stay on disk.");
                ui.separator();
                ui.label(RichText::new("Epic default project folder (fixed)").strong().color(ui.visuals().text_color()));
                ui.label(discovery::default_project_root().to_string_lossy().as_ref());
                for(i,path)in self.prefs.project_roots.iter().enumerate(){
                    if discovery::path_key(Path::new(path))==discovery::path_key(&discovery::default_project_root()){continue;}
                    ui.separator();
                    let (row,_) = ui.allocate_exact_size(vec2(ui.available_width(),42.),Sense::hover());
                    let button_rect=Rect::from_center_size(pos2(row.right()-50.,row.center().y),vec2(100.,COMPACT_BUTTON_HEIGHT));
                    let path_rect=Rect::from_min_max(row.min,pos2(button_rect.left()-14.,row.bottom()));
                    ui.scope_builder(UiBuilder::new().max_rect(path_rect).layout(Layout::left_to_right(Align::Center)),|ui|{
                        ui.add(Label::new(path).truncate()).on_hover_text(path);
                    });
                    if ui.put(button_rect,buttons::secondary_button("Remove")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){remove=Some(i);}
                }
                ui.horizontal_wrapped(|ui|{
                    if ui.add(buttons::secondary_button("Add project folder")).on_hover_text("Choose a folder containing one or many projects, including projects in subfolders.").on_hover_cursor(CursorIcon::PointingHand).clicked(){if let Some(p)=rfd::FileDialog::new().set_title("Choose a folder containing one or more Unreal projects").pick_folder(){self.add_project_root(p,ctx);}}
                    if ui.add(buttons::secondary_button("Find projects on this PC")).clicked(){self.open_project_finder();}
                    if ui.add_enabled(self.scan.receiver.is_none(),buttons::secondary_button("Rescan folders")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.start_scan(ctx.clone(),None);}
                    if self.scan.receiver.is_some(){ui.add(loading::spinner(&theme));ui.label("Scanning project folders…");}
                });
                });
                settings_section(ui,&theme,|ui|{
                ui.heading("Appearance");
                if downloads::toggle(ui, &theme, &mut self.prefs.show_vault_cache_button, "Show Vault Cache in sidebar", "View the cache folder configured in Download settings, including sizes and cached-file removal.") { self.persist(); }
                if downloads::toggle(ui, &theme, &mut self.prefs.show_free_offers_button, "Show Limited-Time Free in sidebar", "Show the Fab limited-time free offers button in the sidebar.") { self.persist(); }
                ui.horizontal_wrapped(|ui|{
                    if ui.add(buttons::secondary_button("Midnight")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.theme=Theme::dark();self.prefs.theme="dark".into();self.theme.apply(ctx);self.persist();}
                    if ui.add(buttons::secondary_button("Daylight")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.theme=Theme::light();self.prefs.theme="light".into();self.theme.apply(ctx);self.persist();}
                });
                });
                settings_section(ui,&theme,|ui|{
                ui.heading("Engine release checks");ui.label("Check for available engine releases while the launcher is open. Updates are always installed manually.");
                ui.horizontal_wrapped(|ui|{
                    ui.label("Check every");if ui.scope(|ui| { buttons::compact_style(ui); ui.add(DragValue::new(&mut self.prefs.check_minutes).range(5..=1440).suffix(" minutes")) }).inner.changed(){self.persist();}
                    if ui.add(buttons::secondary_button("Check now")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.check(ctx.clone());}
                });
                });
                settings_section(ui,&theme,|ui|{
                ui.heading("Plugin updates");
                let busy = self.plugin_work.busy() || self.library.checking_updates() || self.library_work_pending();
                if ui.add_enabled(!busy, buttons::checkbox(&mut self.prefs.wait_for_library_plugin_builds, "Wait for builds listed in the Fab library", &self.theme)).on_disabled_hover_text("Wait for the current plugin check or installation to finish.").changed() {
                    self.persist();
                    self.library.catalogue.clear();
                    self.refresh_library(ctx);
                }
                ui.label("On (default): wait until the compatible package matches the build listed in your Fab library. Off: offer the latest compatible package available from Fab.");
                ui.label("This checks library availability, not publisher approval. If build information is missing, updates wait until it becomes available. Existing installations are unchanged.");
                ui.separator();
                ui.label("A copy of the current plugin is saved before every update. Restore it from Manage plugins → Plugin backups.");
                let before = self.prefs.plugin_backup_days;
                ui.horizontal_wrapped(|ui| {
                    ui.label("Keep new plugin backups");
                    AppComboBox::new(&theme, "plugin-backup-retention")
                        .selected_text(if before == 0 { "Until manually deleted".into() } else { format!("{before} days") })
                        .width(205.)
                        .show_ui(ui, |ui| {
                            for (days, label) in [(7, "7 days (default)"), (14, "14 days"), (30, "30 days"), (0, "Until manually deleted")] {
                                buttons::selectable_value(ui, &mut self.prefs.plugin_backup_days, days, label);
                            }
                        });
                });
                if before != self.prefs.plugin_backup_days { self.persist(); }
                ui.label("Existing backups keep their expiry date. Mark any backup Keep to prevent automatic deletion. Expired backups are removed when you open Plugin backups or update plugins for that engine.");
                });
                settings_section(ui,&theme,|ui|{
                ui.heading("Workspace");ui.horizontal_wrapped(|ui|{
                    if ui.add(buttons::secondary_button("Locate installed engine")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.add_engine();}
                    if ui.add(buttons::secondary_button("Rescan engines")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.engines=platform::display_engines();self.engine_space.checked=None;self.plugin_counts.clear(); self.workspace_cache.invalidate();self.notify(format!("Engine rescan complete: {} installed engines found.",self.engines.len()));}
                });
                });
                settings_section(ui,&theme,|ui|{
                self.desktop_preferences(ui);
                });
                settings_section(ui,&theme,|ui|{self.launcher_update_settings(ui,ctx);});
                ui.label(RichText::new("Independent developer tool. Not affiliated with or endorsed by Epic Games.").color(color(theme.muted)));
                if let Some(e)=&self.error{ui.label(RichText::new(e).color(color(theme.warning)));}
            });
        });
        if let Some(i) = remove {
            self.prefs.project_roots.remove(i);
            self.reconcile_project_roots(ctx);
        }
    }
}

fn settings_section(ui: &mut Ui, theme: &Theme, body: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(color(theme.card))
        .stroke(Stroke::new(1., color(theme.border)))
        .corner_radius(6.)
        .inner_margin(20)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
}
