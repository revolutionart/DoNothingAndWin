use super::*;
impl Workspace {
    pub(super) fn show_plugins(&mut self, i: usize) {
        if self.plugin_work.mutating {
            return;
        }
        self.plugin_work.checked.clear();
        self.plugin_work.status = "Check for updates for this engine version".into();
        self.plugin_engine = Some(i);
        self.plugin_search.clear();
        self.plugin_updates_only = false;
        self.plugins = if self.demo {
            Vec::new()
        } else {
            engine_workspace::plugins::installed(Path::new(&self.engines[i].path))
        };
        if !self.demo {
            self.workspace_cache.invalidate();
            self.plugin_counts
                .insert(self.engines[i].path.clone(), self.plugins.len());
        }
    }
    pub(super) fn engine_card(
        &mut self,
        ui: &mut Ui,
        c: &Canvas,
        i: usize,
        r: [f32; 4],
        compact: bool,
    ) {
        let engine = self.engines[i].clone();
        let [x, y, w, h] = r;
        let selected = if self.prefs.default_engine.is_empty() {
            i == 0
        } else {
            engine.path == self.prefs.default_engine
        };
        let t = self.theme.clone();
        c.boxed(
            ui,
            r,
            t.card,
            if selected { t.accent_hover } else { t.border },
            4.,
            if selected { 1.5 } else { 1. },
        );
        let tx = x + 20.;
        c.text(
            ui,
            tx,
            y + 28.,
            "Unreal Engine",
            if compact { 13. } else { 15. },
            false,
            t.text,
            Align2::LEFT_CENTER,
        );
        let v = EngineVersion::parse(&engine.version)
            .map(|v| v.to_string())
            .unwrap_or(engine.version.clone());
        c.text(
            ui,
            tx,
            y + 58.,
            &v,
            if compact { 26. } else { 29. },
            true,
            t.text,
            Align2::LEFT_CENTER,
        );
        if selected {
            c.boxed(
                ui,
                [x + w - 87., y + 14., 71., 29.],
                t.selection,
                t.border,
                4.,
                1.,
            );
            c.text(
                ui,
                x + w - 51.,
                y + 28.,
                "Current",
                14.,
                false,
                t.accent_hover,
                Align2::CENTER_CENTER,
            );
        }
        c.circle(ui, x + 27., y + 100., 6., t.success);
        c.text(
            ui,
            x + 47.,
            y + 100.,
            if self.demo {
                "Installed"
            } else {
                match self.workspace_cache.engines.get(&engine.path) {
                    Some(true) => "Installed",
                    Some(false) => "Needs repair",
                    None => "Checking…",
                }
            },
            if compact { 12. } else { 15. },
            false,
            t.muted,
            Align2::LEFT_CENTER,
        );
        let count = if self.demo {
            [8, 5, 3][i % 3].to_string()
        } else {
            self.plugin_counts
                .get(&engine.path)
                .map(usize::to_string)
                .unwrap_or_else(|| "…".into())
        };
        c.text(
            ui,
            x + if compact { 108. } else { 122. },
            y + 100.,
            &format!("|   {count} plugins"),
            if compact { 12. } else { 15. },
            false,
            t.muted,
            Align2::LEFT_CENTER,
        );
        if self.demo && i == 0 {
            let bx = x + if compact { 176. } else { 227. };
            c.fill(
                ui,
                [bx, y + 84., if compact { 77. } else { 100. }, 25.],
                t.warning,
                5.,
            );
            c.text(
                ui,
                bx + if compact { 38. } else { 50. },
                y + 96.,
                "2 updates",
                if compact { 12. } else { 14. },
                true,
                t.background,
                Align2::CENTER_CENTER,
            );
        }
        let link = c.rect([x + 16., y + h - 45., if compact { 137. } else { 175. }, 36.]);
        let response = ui
            .interact(link, Id::new(("manage", i)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        c.text(
            ui,
            x + 20.,
            y + h - 27.,
            "Manage plugins  →",
            if compact { 13. } else { 16. },
            false,
            if response.hovered() {
                [255, 255, 255]
            } else {
                t.accent_hover
            },
            Align2::LEFT_CENTER,
        );
        if response
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            self.show_plugins(i);
            self.check_plugins(ui.ctx().clone());
        }
        let menu_rect = button_bounds([x + w - 48., y + h - 50., 32., 38.]);
        let launch_width = if compact { 92. } else { 122. } * BUTTON_SCALE;
        let launch_rect = [
            menu_rect[0] - 8. - launch_width,
            menu_rect[1],
            launch_width,
            menu_rect[3],
        ];
        if c.button(
            ui,
            ("launch", i),
            button_slot(launch_rect),
            "Launch",
            None,
            selected,
        ) {
            self.launch_engine(i);
        }
        let response = ui
            .interact(
                c.rect(menu_rect),
                Id::new(("engine-actions", &engine.path)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        c.boxed(
            ui,
            menu_rect,
            if response.hovered() {
                SIDEBAR_HIGHLIGHT
            } else {
                t.card
            },
            t.border,
            4.,
            1.,
        );
        c.icon(
            ui,
            Icon::Dots,
            menu_rect[0] + 7. * BUTTON_SCALE,
            menu_rect[1] + 10. * BUTTON_SCALE,
            18. * BUTTON_SCALE,
            if response.hovered() {
                SIDEBAR_ON_HIGHLIGHT
            } else {
                t.muted
            },
        );
        egui::Popup::menu(&response)
            .frame(menu_frame(&self.theme))
            .show(|ui| {
                style_menu(ui, &self.theme);
                ui.set_min_width(174.);
                if ui
                    .button("Options")
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.engine_options = Some(engine.clone());
                    ui.close();
                }
                if ui
                    .button("Set Current")
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.prefs.default_engine = engine.path.clone();
                    self.launch_path = Some(engine.path.clone());
                    self.persist();
                    ui.close();
                }
                if ui
                    .add_enabled(!self.demo, Button::new("Create Shortcut"))
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    match platform::create_engine_shortcut(&engine) {
                        Ok(path) => {
                            self.notify(format!("Desktop shortcut created: {}", path.display()))
                        }
                        Err(e) => self.notify(e),
                    }
                    ui.close();
                }
                if ui
                    .add_enabled(!self.demo, Button::new("Show in Folder"))
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    if let Err(e) = platform::open_external(&engine.path) {
                        self.notify(e);
                    }
                    ui.close();
                }
                if ui
                    .button("Manage Plugins")
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.show_plugins(i);
                    self.check_plugins(ui.ctx().clone());
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(
                        !self.demo && !self.transfer.busy() && !self.plugin_work.mutating,
                        Button::new("Remove"),
                    )
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.engine_remove = Some(engine.clone());
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.demo && !self.transfer.busy() && !self.plugin_work.mutating,
                        Button::new("Verify"),
                    )
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.start_maintenance(engine.clone(), false, ui.ctx().clone());
                    ui.close();
                }
            });
    }
    pub(super) fn library_page(&mut self, ui: &mut Ui, c: &Canvas) {
        if !self.demo {
            self.real_library_page(ui, c);
            return;
        }
        let area = c.rect([266., 109., c.width - 292., c.height - 159.]);
        ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
            AppScrollArea::vertical(&self.theme)
                .id_salt("library-scroll")
                .show(ui, |ui| {
                    let local = Canvas {
                        origin: ui.cursor().min - vec2(266., 109.),
                        width: c.width - 12.,
                        height: c.height,
                        sx: 1.,
                        sy: 1.,
                        theme: c.theme.clone(),
                    };
                    let rows = 3_usize.div_ceil(card_columns(local.width - 320., 291., 12.));
                    let height = (110. + rows as f32 * 234.).max(area.height());
                    self.library_section(ui, &local, [266., 109., local.width - 292., height]);
                    ui.allocate_space(vec2(area.width() - 16., height));
                });
        });
    }
    fn library_section(&mut self, ui: &mut Ui, c: &Canvas, r: [f32; 4]) {
        let [x, y, w, _] = r;
        let t = self.theme.clone();
        c.panel(ui, r);
        c.icon(ui, Icon::Chip, x + 17., y + 15., 25., t.text);
        c.text(
            ui,
            x + 59.,
            y + 28.,
            "Library",
            20.,
            true,
            t.text,
            Align2::LEFT_CENTER,
        );
        c.text(
            ui,
            x + 133.,
            y + 28.,
            "Your assets and projects from Fab",
            14.,
            false,
            t.muted,
            Align2::LEFT_CENTER,
        );
        if c.button(
            ui,
            "fab-library",
            [x + w - 110., y + 13., 95., 35.],
            "View all  →",
            None,
            false,
        ) {
            let _ = platform::open_external("https://www.fab.com/library");
        }
        for (j, label) in ["All", "Assets", "Projects", "Plugins"].iter().enumerate() {
            if c.pill(
                ui,
                &format!("librarytab{j}"),
                [x + 17. + j as f32 * 82., y + 52., 76., 29.],
                label,
                j == 0,
            ) {
                self.notify("Fab account library integration is not connected in this build.");
            }
        }
        if !self.demo {
            c.text(
                ui,
                x + 25.,
                y + 123.,
                "Connect your developer library",
                22.,
                true,
                t.text,
                Align2::LEFT_CENTER,
            );
            c.text(ui,x+25.,y+162.,"Fab library synchronization is not implemented yet. Your installed plugins are available under Manage plugins.",14.,false,t.muted,Align2::LEFT_CENTER);
            if c.button(
                ui,
                "browsefab",
                [x + 25., y + 198., 195., 40.],
                "Open Fab library",
                Some(Icon::Book),
                true,
            ) {
                let _ = platform::open_external("https://www.fab.com/library");
            }
            return;
        }
        let cardw = 291.;
        let columns = card_columns(w - 28., cardw, 12.);
        for (i, (title, description, art, action, kind)) in [
            (
                "Forest Environment",
                "High-quality forests, foliage and natural assets.",
                "library-forest",
                "Add to project",
                "Asset",
            ),
            (
                "Archviz Starter",
                "Complete archviz project with assets and lighting.",
                "library-archviz",
                "Create project",
                "Project",
            ),
            (
                "Terrain Tools",
                "Advanced terrain creation and editing tools.",
                "library-terrain",
                "Install to engine",
                "Plugin",
            ),
        ]
        .iter()
        .enumerate()
        {
            let cx = x + 14. + (i % columns) as f32 * (cardw + 12.);
            let cy = y + 91. + (i / columns) as f32 * 234.;
            c.boxed(ui, [cx, cy, cardw, 222.], t.card, t.border, 4., 1.);
            if let Some(tex) = self.textures.get(*art) {
                c.image(
                    ui,
                    tex,
                    [cx + 1., cy + 1., cardw - 2., 120.],
                    [255, 255, 255],
                );
            }
            c.text(
                ui,
                cx + 13.,
                cy + 139.,
                title,
                16.,
                true,
                t.text,
                Align2::LEFT_CENTER,
            );
            c.text(
                ui,
                cx + 13.,
                cy + 160.,
                description,
                12.,
                false,
                t.muted,
                Align2::LEFT_CENTER,
            );
            c.text(
                ui,
                cx + 14.,
                cy + 197.,
                "From Fab",
                12.,
                false,
                t.muted,
                Align2::LEFT_CENTER,
            );
            c.fill(ui, [cx + cardw - 86., cy + 7., 78., 27.], t.card, 4.);
            c.text(
                ui,
                cx + cardw - 47.,
                cy + 20.,
                kind,
                13.,
                true,
                t.text,
                Align2::CENTER_CENTER,
            );
            if c.button(
                ui,
                ("sampleasset", i),
                [cx + cardw - 176., cy + 178., 162., 36.],
                action,
                None,
                true,
            ) {
                self.notify("Sample library item. Open Fab to manage your real assets.");
            }
        }
    }
}
