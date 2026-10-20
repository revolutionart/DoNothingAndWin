use super::*;
impl Workspace {
    pub(super) fn has_engine_updates(&self) -> bool {
        engine_updates_available(&self.engines, &self.snapshot.releases, |engine| {
            if self.demo {
                Some(true)
            } else {
                self.workspace_cache.engines.get(&engine.path).copied()
            }
        })
    }

    pub(super) fn show_plugins(&mut self, i: usize) {
        if self.plugin_work.mutating {
            return;
        }
        let checked = self.library.catalogue.rows(&self.engines[i].path);
        self.plugin_work.status = if checked.is_empty() {
            "Check for updates for this engine version".into()
        } else {
            engine_workspace::plugins::check_summary(checked)
        };
        self.plugin_engine = Some(i);
        self.plugin_search.clear();
        self.plugin_updates_only = false;
        self.plugins = if self.demo {
            Vec::new()
        } else {
            engine_workspace::plugins::installed(Path::new(&self.engines[i].path))
        };
        // Never show an old comparison against a newly edited/replaced descriptor.
        self.library
            .catalogue
            .retain_installed(&self.engines[i].path, &self.plugins);
        if !self.demo {
            self.workspace_cache.invalidate();
            self.plugin_counts
                .insert(self.engines[i].path.clone(), self.plugins.len());
        }
    }
    pub(super) fn engine_card(&mut self, ui: &mut Ui, c: &Canvas, i: usize, r: [f32; 4]) {
        let engine = self.engines[i].clone();
        let installed = self.workspace_cache.engines.get(&engine.path).copied();
        let (status, update) = engine_card_status(
            if self.demo { Some(true) } else { installed },
            &engine,
            &self.snapshot.releases,
        );
        let update_version = update
            .and_then(|r| r.version.as_ref())
            .map(ToString::to_string);
        let update_release = update.cloned();
        let status = update_version
            .as_ref()
            .map(|version| format!("Update available - UE{version}"))
            .unwrap_or_else(|| status.to_owned());
        let [x, y, w, h] = r;
        let status_y = y + 69.;
        let selected = if self.prefs.default_engine.is_empty() {
            i == 0
        } else {
            engine.path == self.prefs.default_engine
        };
        let card_response = ui.interact(
            c.rect(r),
            Id::new(("engine-card", &engine.path)),
            Sense::click(),
        );
        let t = self.theme.clone();
        c.boxed(
            ui,
            r,
            t.card,
            if update_version.is_some() {
                t.warning
            } else if selected {
                t.accent_hover
            } else {
                t.border
            },
            6.,
            if selected { 1.5 } else { 1. },
        );
        let tx = x + 18.;
        let heading_y = y + 30.;
        c.text(
            ui,
            tx,
            heading_y,
            "Unreal Engine",
            14.,
            false,
            t.text,
            Align2::LEFT_CENTER,
        );
        let v = EngineVersion::parse(&engine.version)
            .map(|v| v.to_string())
            .unwrap_or(engine.version.clone());
        let label_width = ui
            .painter()
            .layout_no_wrap(
                "Unreal Engine".into(),
                FontId::proportional(14. * c.scale()),
                color(t.text),
            )
            .size()
            .x
            / c.scale();
        let version_x = tx + label_width + 18.;
        let version_right = x + w - if selected { 100. } else { 18. };
        let version_rect = c.rect([version_x, y + 8., version_right - version_x, 44.]);
        let version_painter = ui.painter().with_clip_rect(version_rect);
        version_painter.text(
            c.point(version_x, heading_y),
            Align2::LEFT_CENTER,
            &v,
            FontId::new(28. * c.scale(), FontFamily::Name("semibold".into())),
            color(t.text),
        );
        ui.interact(
            version_rect,
            Id::new(("engine-version-tip", i)),
            Sense::hover(),
        )
        .on_hover_text(&engine.version);
        if selected {
            c.fill(ui, [x + w - 86., y + 15., 70., 30.], t.selection, 5.);
            c.text(
                ui,
                x + w - 51.,
                heading_y,
                "Default",
                14.,
                false,
                t.accent_hover,
                Align2::CENTER_CENTER,
            );
        }
        let status_color = if update_version.is_some() {
            t.warning
        } else if self.demo || installed == Some(true) {
            t.success
        } else if installed == Some(false) {
            t.warning
        } else {
            t.muted
        };
        c.circle(ui, tx + 7., status_y, 7., status_color);
        let status_width = ui
            .painter()
            .layout_no_wrap(
                status.clone(),
                FontId::proportional(14. * c.scale()),
                color(t.muted),
            )
            .size()
            .x
            / c.scale();
        let mut status_text_color = t.muted;
        if let Some(version) = &update_version {
            let response = ui
                .interact(
                    c.rect([tx + 24., status_y - 14., status_width + 6., 28.]),
                    Id::new(("engine-update", &engine.path)),
                    Sense::click(),
                )
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(format!(
                    "Unreal Engine {version} is available. View engine releases."
                ));
            status_text_color = if response.hovered() {
                t.text
            } else {
                t.warning
            };
            if response.clicked() {
                self.page = Page::Engines;
            }
        }
        c.text(
            ui,
            tx + 27.,
            status_y,
            &status,
            14.,
            false,
            status_text_color,
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
            tx + 27. + status_width + 12.,
            status_y,
            &format!("|   {count} plugins"),
            14.,
            false,
            t.muted,
            Align2::LEFT_CENTER,
        );
        let action_y = y + h - 46.;
        let has_plugin_updates = self
            .library
            .catalogue
            .rows(&engine.path)
            .iter()
            .any(|row| row.has_update());
        let menu_rect = [x + w - 46., action_y, 30., 32.];
        let launch_rect = [menu_rect[0] - 92., action_y, 86., 32.];
        let link = c.rect([tx - 4., action_y, launch_rect[0] - tx - 8., 32.]);
        let response = ui
            .interact(link, Id::new(("manage", i)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let link_color = if response.hovered() {
            t.text
        } else if has_plugin_updates {
            t.warning
        } else {
            t.accent_hover
        };
        c.text(
            ui,
            x + 20.,
            action_y + 16.,
            if has_plugin_updates {
                "Manage plugins"
            } else {
                "Manage plugins  →"
            },
            14.,
            false,
            link_color,
            Align2::LEFT_CENTER,
        );
        if has_plugin_updates {
            let label_width = ui
                .painter()
                .layout_no_wrap(
                    "Manage plugins".into(),
                    FontId::proportional(14. * c.scale()),
                    color(link_color),
                )
                .size()
                .x
                / c.scale();
            let badge_x = x + 20. + label_width + 16.;
            ui.painter().circle_stroke(
                c.point(badge_x, action_y + 16.),
                8. * c.scale(),
                Stroke::new(1.5, color(link_color)),
            );
            c.text(
                ui,
                badge_x,
                action_y + 16.,
                "!",
                13.,
                true,
                link_color,
                Align2::CENTER_CENTER,
            );
        }
        if response
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            self.show_plugins(i);
            self.check_plugins(ui.ctx().clone());
        }
        let action_clicked = if update_release.is_some() {
            c.update_button(ui, ("update-engine", i), button_slot(launch_rect))
        } else {
            c.button(
                ui,
                ("launch", i),
                button_slot(launch_rect),
                "Launch",
                None,
                selected,
            )
        };
        if action_clicked {
            if let Some(release) = update_release {
                self.start_engine_update(engine.clone(), release, ui.ctx().clone());
            } else {
                self.launch_engine(i);
            }
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
            menu_rect[0] + 7.,
            menu_rect[1] + 8.,
            16.,
            if response.hovered() {
                SIDEBAR_ON_HIGHLIGHT
            } else {
                t.muted
            },
        );
        egui::Popup::menu(&response)
            .frame(menu_frame(&self.theme))
            .show(|ui| self.engine_actions_menu(ui, i));
        egui::Popup::context_menu(&card_response)
            .frame(menu_frame(&self.theme))
            .show(|ui| self.engine_actions_menu(ui, i));
    }
    fn engine_actions_menu(&mut self, ui: &mut Ui, i: usize) {
        let engine = self.engines[i].clone();
        style_menu(ui, &self.theme);
        ui.set_min_width(174.);
        if ui
            .add(buttons::secondary_button("Options"))
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            self.engine_options = Some(engine.clone());
            ui.close();
        }
        if ui
            .add(buttons::secondary_button("Set as default"))
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
            .add_enabled(!self.demo, buttons::secondary_button("Create Shortcut"))
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            match platform::create_engine_shortcut(&engine) {
                Ok(path) => self.notify(format!("Desktop shortcut created: {}", path.display())),
                Err(e) => self.notify(e),
            }
            ui.close();
        }
        if ui
            .add_enabled(!self.demo, buttons::secondary_button("Show in Folder"))
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
            .add(buttons::secondary_button("Manage Plugins"))
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
                buttons::secondary_button("Remove"),
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
                buttons::secondary_button("Verify"),
            )
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            self.start_maintenance(engine.clone(), false, ui.ctx().clone());
            ui.close();
        }
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
                button_slot([
                    x + 17. + j as f32 * 82.,
                    y + 52.,
                    76.,
                    COMPACT_BUTTON_HEIGHT,
                ]),
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

fn engine_updates_available(
    engines: &[Installation],
    releases: &[Release],
    ready: impl Fn(&Installation) -> Option<bool>,
) -> bool {
    engines.iter().any(|engine| {
        engine_card_status(ready(engine), engine, releases)
            .1
            .is_some_and(|release| {
                // The sidebar describes available releases, like the Engines table.
                // A retained older copy can still offer its own Update action without
                // announcing a release that is already installed in this branch.
                !engines.iter().any(|other| {
                    ready(other) == Some(true)
                        && EngineVersion::parse(&other.version)
                            .zip(release.version.as_ref())
                            .is_some_and(|(installed, available)| {
                                installed.major == available.major
                                    && installed.minor == available.minor
                                    && installed >= *available
                            })
                })
            })
    })
}

// Shared by the engine cards, sidebar badge and regression tests.
fn engine_card_status<'a>(
    installed: Option<bool>,
    engine: &Installation,
    releases: &'a [Release],
) -> (&'static str, Option<&'a Release>) {
    match installed {
        None => return ("Checking…", None),
        Some(false) => return ("Needs repair", None),
        Some(true) => {}
    }
    let update = EngineVersion::parse(&engine.version).and_then(|installed| {
        releases
            .iter()
            .filter(|release| {
                release.platform == "Windows"
                    && release.channel == "Live"
                    && release
                        .build
                        .as_ref()
                        .is_some_and(|build| !build.is_empty())
                    && EngineVersion::parse(&release.app).is_some_and(|branch| {
                        branch.major == installed.major && branch.minor == installed.minor
                    })
                    && release.version.as_ref().is_some_and(|version| {
                        version.major == installed.major
                            && version.minor == installed.minor
                            && version > &installed
                    })
            })
            .max_by(|a, b| a.version.cmp(&b.version))
    });
    if update.is_some() {
        ("Update available", update)
    } else {
        ("Installed", None)
    }
}

#[cfg(test)]
mod engine_card_tests {
    use super::*;

    #[test]
    fn engine_badge_clears_after_update_with_an_older_copy_retained() {
        let mut engines = vec![Installation {
            version: "5.8.3".into(),
            path: "F:/Engines/UE_5.8".into(),
            association: "5.8".into(),
        }];
        let releases = vec![Release {
            app: "UE_5.8".into(),
            catalog_id: "engine-58".into(),
            platform: "Windows".into(),
            channel: "Live".into(),
            build: Some("5.8.4-58902052+++UE5+Release-5.8-Windows".into()),
            version: EngineVersion::parse("5.8.4"),
        }];
        assert!(engine_updates_available(&engines, &releases, |_| Some(
            true
        )));
        engines.push(Installation {
            path: "F:/Engines/UE_5.8-5.8.3-Workspace".into(),
            association: "{CUSTOM-GUID}".into(),
            ..engines[0].clone()
        });
        engines[0].version = "5.8.4".into();
        assert!(
            !engine_updates_available(&engines, &releases, |_| Some(true)),
            "The engine release is installed; retaining an older copy must not keep its sidebar alert lit"
        );
        assert!(
            engine_card_status(Some(true), &engines[1], &releases)
                .1
                .is_some()
        );
        // A broken newer installation must not suppress an actionable update.
        assert!(engine_updates_available(&engines, &releases, |engine| {
            Some(engine.path != engines[0].path)
        }));
        engines[0].version = "5.8.5".into();
        assert!(!engine_updates_available(&engines, &releases, |_| Some(
            true
        )));
        // Updating one branch must not clear the alert for another branch.
        engines.push(Installation {
            version: "5.7.3".into(),
            path: "F:/Engines/UE_5.7".into(),
            association: "5.7".into(),
        });
        let mut releases = releases;
        releases.push(Release {
            app: "UE_5.7".into(),
            version: EngineVersion::parse("5.7.4"),
            build: Some("5.7.4-build".into()),
            ..releases[0].clone()
        });
        assert!(engine_updates_available(&engines, &releases, |_| Some(
            true
        )));
        engines[2].version = "5.7.4".into();
        assert!(!engine_updates_available(&engines, &releases, |_| Some(
            true
        )));
    }

    #[test]
    fn workspace_card_only_offers_newer_compatible_resolved_live_releases() {
        let engine = Installation {
            version: "5.8.3".into(),
            path: "engine".into(),
            association: "5.8".into(),
        };
        let release = Release {
            app: "UE_5.8".into(),
            catalog_id: "engine-58".into(),
            platform: "Windows".into(),
            channel: "Live".into(),
            build: Some("5.8.4-58902052+++UE5+Release-5.8-Windows".into()),
            version: EngineVersion::parse("5.8.4"),
        };
        assert_eq!(
            engine_card_status(None, &engine, &[release.clone()]).0,
            "Checking…"
        );
        assert_eq!(
            engine_card_status(Some(false), &engine, &[release.clone()]).0,
            "Needs repair"
        );
        let updated = Installation {
            version: "5.8.4".into(),
            ..engine.clone()
        };
        assert_eq!(
            engine_card_status(Some(true), &updated, &[release.clone()]).0,
            "Installed"
        );
        for invalid in [
            Release {
                version: EngineVersion::parse("5.8.2"),
                ..release.clone()
            },
            Release {
                app: "UE_5.9".into(),
                version: EngineVersion::parse("5.9.0"),
                ..release.clone()
            },
            Release {
                build: None,
                ..release.clone()
            },
            Release {
                channel: "Preview".into(),
                ..release.clone()
            },
            Release {
                platform: "Mac".into(),
                ..release.clone()
            },
        ] {
            assert_eq!(
                engine_card_status(Some(true), &engine, &[invalid]).0,
                "Installed"
            );
        }
        assert_eq!(engine_card_status(Some(true), &engine, &[]).0, "Installed");
    }

    #[test]
    fn workspace_card_reports_hotfix_already_detected_in_releases() {
        let engine = Installation {
            version: "5.8.3".into(),
            path: "F:/Engines/UE_5.8".into(),
            association: "5.8".into(),
        };
        let releases = vec![Release {
            app: "UE_5.8".into(),
            catalog_id: "engine-58".into(),
            platform: "Windows".into(),
            channel: "Live".into(),
            build: Some("5.8.4-58902052+++UE5+Release-5.8-Windows".into()),
            version: EngineVersion::parse("5.8.4"),
        }];
        let (label, update) = engine_card_status(Some(true), &engine, &releases);
        assert_eq!(label, "Update available");
        assert_eq!(
            update.unwrap().version.as_ref().unwrap().to_string(),
            "5.8.4"
        );
    }
}
