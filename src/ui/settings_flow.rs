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
        if !path.is_dir() {
            self.notify("Choose an existing project folder.");
            return;
        }
        let key = discovery::path_key(&path);
        if discovery::project_roots(&self.prefs.project_roots)
            .iter()
            .any(|p| discovery::path_key(Path::new(p)) == key)
        {
            self.notify("This project folder is already configured.");
            return;
        }
        self.prefs.project_roots.push(
            path.to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_owned(),
        );
        self.reconcile_project_roots(ctx);
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
        let area = c.rect([271., 118., c.width - 297., c.height - 180.]);
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
                ui.label("Projects in these folders and their subfolders appear in Projects. Removing a folder only removes its projects from this launcher; files stay on disk.");
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
                    if ui.add(buttons::secondary_button("Add project folder")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){if let Some(p)=rfd::FileDialog::new().pick_folder(){self.add_project_root(p,ctx);}}
                    if ui.add_enabled(self.scan.receiver.is_none(),buttons::secondary_button("Rescan folders")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.start_scan(ctx.clone(),None);}
                    if self.scan.receiver.is_some(){ui.spinner();ui.label("Scanning project folders…");}
                });
                });
                settings_section(ui,&theme,|ui|{
                ui.heading("Appearance");
                ui.horizontal_wrapped(|ui|{
                    if ui.add(buttons::secondary_button("Midnight")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.theme=Theme::dark();self.prefs.theme="dark".into();ctx.set_visuals(self.theme.visuals());self.persist();}
                    if ui.add(buttons::secondary_button("Daylight")).clone().on_hover_cursor(CursorIcon::PointingHand).clicked(){self.theme=Theme::light();self.prefs.theme="light".into();ctx.set_visuals(self.theme.visuals());self.persist();}
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
