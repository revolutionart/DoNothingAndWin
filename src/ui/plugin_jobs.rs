use super::*;
use engine_workspace::plugins::{self, Checked, Plugin, Update};
enum Event {
    Checked(String, Result<Vec<Checked>, String>),
    Finished(String, Result<String, String>),
}
#[derive(Default)]
pub(super) struct PluginWork {
    rx: Option<Receiver<Event>>,
    pub mutating: bool,
    pub checked: Vec<Checked>,
    pub update_engines: std::collections::HashMap<String, bool>,
    pub status: String,
    pub icons_loaded: std::collections::HashSet<String>,
    confirm: Option<(Installation, Plugin)>,
}
impl Workspace {
    pub(super) fn check_plugins(&mut self, ctx: Context) {
        if self.plugin_work.rx.is_some() || self.demo {
            return;
        }
        let Some(index) = self.plugin_engine else {
            return;
        };
        let engine = self.engines[index].clone();
        let Some(session) = self.session.clone() else {
            self.plugin_work.status = "Connect Epic to check updates".into();
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.plugin_work.rx = Some(rx);
        self.plugin_work.status = "Checking compatible plugin builds…".into();
        std::thread::spawn(move || {
            let result = plugins::check(&engine, &session);
            let _ = tx.send(Event::Checked(engine.path, result));
            ctx.request_repaint();
        });
    }
    pub(super) fn poll_plugins(&mut self, ctx: &Context) {
        let event = self
            .plugin_work
            .rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if self.plugin_work.rx.is_some() && !crate::window_frame::is_backgrounded() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        if let Some(event) = event {
            self.plugin_work.rx = None;
            self.plugin_work.mutating = false;
            match event {
                Event::Checked(path, result) => {
                    if let Ok(items) = &result {
                        self.plugin_work
                            .update_engines
                            .insert(path.clone(), items.iter().any(|item| item.update.is_some()));
                    }
                    if self
                        .plugin_engine
                        .is_some_and(|i| self.engines[i].path == path)
                    {
                        match result {
                            Ok(items) => {
                                self.plugin_work.checked = items;
                                self.plugin_work.status = "Update check complete".into();
                            }
                            Err(e) => self.plugin_work.status = e,
                        }
                    } else if self.plugin_engine.is_some() {
                        // A different engine was selected while the previous check ran.
                        self.check_plugins(ctx.clone());
                    }
                }
                Event::Finished(path, result) => {
                    let version = self
                        .engines
                        .iter()
                        .find(|e| e.path == path)
                        .map(|e| e.version.clone())
                        .unwrap_or_default();
                    self.record_activity(self.plugin_work.status.clone(), version, &result);
                    self.plugin_counts.remove(&path);
                    self.workspace_cache.invalidate();
                    let message =
                        result.unwrap_or_else(|e| format!("Plugin operation failed: {e}"));
                    self.notify(message.clone());
                    self.plugin_work.status = message;
                    if let Some(i) = self.plugin_engine {
                        self.show_plugins(i);
                        self.check_plugins(ctx.clone());
                    }
                }
            }
        }
    }
    pub(super) fn run_plugin_updates(&mut self, items: Vec<(Plugin, Update)>, ctx: Context) {
        if self.plugin_work.rx.is_some() || self.transfer.busy() || self.demo {
            return;
        }
        let Some(i) = self.plugin_engine else {
            return;
        };
        let engine = self.engines[i].clone();
        let (tx, rx) = mpsc::channel();
        self.plugin_work.rx = Some(rx);
        self.plugin_work.mutating = true;
        self.plugin_work.status = format!("Updating {} plugin(s)…", items.len());
        std::thread::spawn(move || {
            let result = (|| {
                let mut messages = vec![];
                for (p, u) in items {
                    match plugins::update_plugin(&engine, &p, &u) {
                        Ok(m) => messages.push(m),
                        Err(e) => {
                            return Err(format!(
                                "{}; completed {} updates before stopping",
                                e,
                                messages.len()
                            ));
                        }
                    }
                }
                Ok(messages.join("\n"))
            })();
            let _ = tx.send(Event::Finished(engine.path, result));
            ctx.request_repaint();
        });
    }
    pub(super) fn plugin_confirmation(&mut self, ctx: &Context) {
        let Some((engine, plugin)) = self.plugin_work.confirm.clone() else {
            return;
        };
        let mut open = true;
        let mut yes = false;
        dialog::show(
            ctx,
            &self.theme.clone(),
            "remove-plugin",
            "Remove plugin",
            "Remove this plugin from the selected engine.",
            &mut open,
            |ui| {
                ui.label(format!("Remove {} from Unreal Engine {}? Projects that depend on it will need it reinstalled.",plugin.name,engine.version));
                dialog::info(
                    ui,
                    &self.theme,
                    "Plugin folder",
                    &plugin.descriptor.parent().unwrap().display().to_string(),
                );
                ui.label("The plugin files will be deleted from this engine. Projects and other engine versions are unaffected.");
                yes = dialog::footer(
                    ui,
                    &self.theme,
                    "Remove plugin",
                    self.plugin_work.rx.is_none() && !self.transfer.busy(),
                    true,
                );
            },
        );
        if yes {
            self.plugin_work.confirm = None;
            let (tx, rx) = mpsc::channel();
            self.plugin_work.rx = Some(rx);
            self.plugin_work.mutating = true;
            self.plugin_work.status = format!("Removing {}…", plugin.name);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = plugins::remove_plugin(&engine, &plugin);
                let _ = tx.send(Event::Finished(engine.path, result));
                ctx.request_repaint();
            });
        } else if !open {
            self.plugin_work.confirm = None;
        }
    }
    pub(super) fn plugin_dialog(&mut self, ctx: &Context) {
        let Some(index) = self.plugin_engine.filter(|i| *i < self.engines.len()) else {
            return;
        };
        let t = self.theme.clone();
        let engine = self.engines[index].clone();
        for p in &self.plugins {
            let key = p.descriptor.to_string_lossy().into_owned();
            if self.plugin_work.icons_loaded.insert(key.clone()) {
                if let Ok(img) =
                    image::open(p.descriptor.parent().unwrap().join("Resources/Icon128.png"))
                {
                    let rgba = img.to_rgba8();
                    self.textures.insert(
                        key.clone(),
                        ctx.load_texture(
                            key,
                            ColorImage::from_rgba_unmultiplied(
                                [rgba.width() as usize, rgba.height() as usize],
                                &rgba,
                            ),
                            TextureOptions::LINEAR,
                        ),
                    );
                }
            }
        }
        let width = 1180f32.min(ctx.content_rect().width() - 96.);
        let busy = self.plugin_work.rx.is_some() || self.transfer.busy();
        let updates: Vec<_> = self
            .plugin_work
            .checked
            .iter()
            .filter_map(|i| i.update.clone().map(|u| (i.plugin.clone(), u)))
            .collect();
        let mut close = false;
        let mut next = index;
        let response = Modal::new(Id::new("engine-plugins-dialog"))
            .backdrop_color(Color32::from_black_alpha(175))
            .frame(
                Frame::new()
                    .fill(color(t.sidebar))
                    .stroke(Stroke::NONE)
                    .corner_radius(10.)
                    .inner_margin(24),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = vec2(12., 12.);
                ui.spacing_mut().button_padding = vec2(16., 9.);
                ui.style_mut().override_font_id = Some(FontId::proportional(16.));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Engine plugins")
                            .size(27.)
                            .strong()
                            .color(ui.visuals().text_color()),
                    );
                    ui.add_enabled_ui(!self.plugin_work.mutating, |ui| {
                        egui::ComboBox::from_id_salt("plugin-dialog-engine")
                            .selected_text(format!("UE {}", engine.version))
                            .width(130.)
                            .show_ui(ui, |ui| {
                                for (i, e) in self.engines.iter().enumerate() {
                                    ui.selectable_value(&mut next, i, format!("UE {}", e.version));
                                }
                            });
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close |= ui
                            .button("×")
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked();
                    });
                });
                ui.label(
                    RichText::new(format!(
                        "Manage plugins installed for Unreal Engine {}.",
                        engine.version
                    ))
                    .color(color(t.muted)),
                );
                ui.label(RichText::new("Changes apply to this engine only.").color(color(t.muted)));
                ui.add_space(4.);
                ui.horizontal(|ui| {
                    for (flag, label) in [
                        (false, format!("Installed ({})", self.plugins.len())),
                        (true, format!("Updates ({})", updates.len())),
                    ] {
                        let selected = self.plugin_updates_only == flag;
                        let r = ui
                            .selectable_label(selected, label)
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if r.clone()
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.plugin_updates_only = flag;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if updates.is_empty() {
                            "Check updates".into()
                        } else {
                            format!("Update all ({})", updates.len())
                        };
                        if ui
                            .scope(|ui| {
                                ui.visuals_mut().widgets.inactive.fg_stroke.color =
                                    color(t.warning);
                                ui.add_enabled(
                                    !busy,
                                    Button::new(label).stroke(Stroke::new(1., color(t.warning))),
                                )
                            })
                            .inner
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked()
                        {
                            if updates.is_empty() {
                                self.check_plugins(ctx.clone());
                            } else {
                                self.run_plugin_updates(updates.clone(), ctx.clone());
                            }
                        }
                        if ui
                            .add(
                                Button::new(
                                    RichText::new("+  Install from Fab").color(color(t.on_accent)),
                                )
                                .fill(color(t.accent)),
                            )
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.page = Page::Library;
                            self.library.filter = 2;
                            close = true;
                        }
                    });
                });
                ui.separator();
                search_field(
                    ui,
                    &mut self.plugin_search,
                    "Search installed plugins...",
                    width,
                    &t,
                    1.,
                );
                let table_width = width - 12.;
                let columns = [
                    0.,
                    table_width * 0.34,
                    table_width * 0.46,
                    table_width * 0.58,
                    table_width * 0.80,
                ];
                let (header, _) = ui.allocate_exact_size(vec2(table_width, 26.), Sense::hover());
                for (i, label) in ["Plugin", "Installed", "Available", "Status", "Actions"]
                    .iter()
                    .enumerate()
                {
                    ui.painter().text(
                        header.min + vec2(columns[i] + 8., 13.),
                        Align2::LEFT_CENTER,
                        label,
                        FontId::proportional(15.),
                        color(t.muted),
                    );
                }
                let rows = self.plugins.clone();
                let mut shown = 0;
                let list_area = AppScrollArea::vertical(&t)
                    .id_salt("plugin-dialog-list")
                    .max_height((ctx.content_rect().height() - 520.).clamp(120., 400.))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.;
                        // Keep room for the spinner and wrapped status even with no rows.
                        ui.set_min_height(120.);
                        let mut row_borders = Vec::new();
                        for p in rows {
                            let checked = self
                                .plugin_work
                                .checked
                                .iter()
                                .find(|i| i.plugin.descriptor == p.descriptor)
                                .cloned();
                            let update = checked.as_ref().and_then(|i| i.update.clone());
                            if !p
                                .name
                                .to_lowercase()
                                .contains(&self.plugin_search.to_lowercase())
                                || (self.plugin_updates_only && update.is_none())
                            {
                                continue;
                            }
                            shown += 1;
                            let (row, _) =
                                ui.allocate_exact_size(vec2(table_width, 58.), Sense::hover());
                            // Allocate the row once; cell and button layouts must not move
                            // the list cursor or change the next row's bounds.
                            let mut row_ui = ui.new_child(
                                UiBuilder::new()
                                    .id_salt(("plugin-row", &p.descriptor))
                                    .max_rect(row)
                                    .layout(Layout::left_to_right(Align::Center)),
                            );
                            let ui = &mut row_ui;
                            let hovered = ui.rect_contains_pointer(row);
                            ui.painter().rect_filled(
                                row,
                                0.,
                                color(if hovered {
                                    if t.dark {
                                        [29, 34, 38]
                                    } else {
                                        [232, 236, 241]
                                    }
                                } else {
                                    t.card
                                }),
                            );
                            let image_rect =
                                Rect::from_min_size(row.min + vec2(8., 9.5), vec2(39., 39.));
                            let key = p.descriptor.to_string_lossy().into_owned();
                            if let Some(tex) = self.textures.get(&key) {
                                ui.painter().image(
                                    tex.id(),
                                    image_rect,
                                    Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                                    Color32::WHITE,
                                );
                            } else {
                                ui.painter().rect_filled(image_rect, 5., color(t.selection));
                                ui.painter().text(
                                    image_rect.center(),
                                    Align2::CENTER_CENTER,
                                    "P",
                                    FontId::proportional(18.),
                                    color(t.muted),
                                );
                            }
                            let cell = |start: f32, end: f32| {
                                Rect::from_min_max(
                                    row.min + vec2(start + 8., 10.),
                                    pos2(row.left() + end - 8., row.bottom() - 10.),
                                )
                            };
                            plugin_cell(
                                ui,
                                cell(47., columns[1]),
                                RichText::new(&p.name)
                                    .strong()
                                    .color(ui.visuals().text_color()),
                                &p.name,
                            );
                            plugin_cell(
                                ui,
                                cell(columns[1], columns[2]),
                                RichText::new(&p.version),
                                &p.version,
                            );
                            let available =
                                update.as_ref().map(|u| u.version.as_str()).unwrap_or("—");
                            plugin_cell(
                                ui,
                                cell(columns[2], columns[3]),
                                RichText::new(available),
                                available,
                            );
                            let detail = checked
                                .as_ref()
                                .map(|i| i.status.as_str())
                                .unwrap_or("Not checked");
                            let status = checked
                                .as_ref()
                                .map(|item| item.status_label())
                                .unwrap_or("Not checked");
                            let col = if update.is_some() {
                                t.warning
                            } else if matches!(status, "Up to date" | "Installed newer") {
                                t.success
                            } else {
                                t.muted
                            };
                            let status_rect = cell(columns[3], columns[4]);
                            ui.painter().circle_filled(
                                pos2(status_rect.left() + 4., row.center().y),
                                4.,
                                color(col),
                            );
                            plugin_cell(
                                ui,
                                Rect::from_min_max(
                                    status_rect.min + vec2(16., 0.),
                                    status_rect.max,
                                ),
                                RichText::new(status).color(color(col)),
                                detail,
                            );
                            let action = cell(columns[4], table_width);
                            ui.add_enabled_ui(!busy, |ui| {
                                ui.spacing_mut().button_padding = vec2(6., 4.);
                                ui.spacing_mut().interact_size.y = 30.;
                                let bw = (action.width() - 8.) / 2.;
                                let update_rect = Rect::from_min_size(
                                    pos2(action.left(), row.center().y - 15.),
                                    vec2(bw, 30.),
                                );
                                let remove_rect = update_rect.translate(vec2(bw + 8., 0.));
                                if let Some(update) = update {
                                    if ui
                                        .put(
                                            update_rect,
                                            Button::new(
                                                RichText::new("Update").color(color(t.on_accent)),
                                            )
                                            .fill(color(t.accent)),
                                        )
                                        .on_hover_cursor(CursorIcon::PointingHand)
                                        .clicked()
                                    {
                                        self.run_plugin_updates(
                                            vec![(p.clone(), update)],
                                            ctx.clone(),
                                        );
                                    }
                                }
                                let remove = ui
                                    .put(
                                        remove_rect,
                                        Button::new("").stroke(Stroke::new(1., color(t.danger))),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                                remove.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Button,
                                        ui.is_enabled(),
                                        "Remove",
                                    )
                                });
                                // Optical centering compensates for the font's lower baseline.
                                ui.painter().text(
                                    remove.rect.center() - vec2(0., 1.5),
                                    Align2::CENTER_CENTER,
                                    "Remove",
                                    egui::TextStyle::Button.resolve(ui.style()),
                                    if remove.hovered() || remove.is_pointer_button_down_on() {
                                        Color32::BLACK
                                    } else {
                                        color(t.danger)
                                    },
                                );
                                if remove
                                    .clone()
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    self.plugin_work.confirm = Some((engine.clone(), p.clone()));
                                }
                            });
                            row_borders.push(row);
                        }
                        // Paint separators last so adjacent row backgrounds cannot cover them.
                        for row in row_borders {
                            ui.painter().line_segment(
                                [
                                    row.left_bottom() - vec2(0., 1.),
                                    row.right_bottom() - vec2(0., 1.),
                                ],
                                Stroke::new(
                                    1.,
                                    color(if t.dark {
                                        [76, 85, 94]
                                    } else {
                                        [180, 190, 202]
                                    }),
                                ),
                            );
                        }
                        if shown == 0 {
                            ui.add_space(20.);
                            ui.label(if self.plugins.is_empty() {
                                "No separately installed plugins for this engine."
                            } else {
                                "No plugins match this view."
                            });
                            ui.add_space(20.);
                        }
                    });
                if self.plugin_work.rx.is_some() {
                    // egui returns the viewport before auto-shrink. Match the actual
                    // reserved height so the overlay cannot cover the count/footer.
                    let rect = Rect::from_min_size(
                        list_area.inner_rect.min,
                        vec2(
                            list_area.inner_rect.width(),
                            list_area.inner_rect.height().min(list_area.content_size.y),
                        ),
                    );
                    ui.painter()
                        .rect_filled(rect, 0., Color32::from_black_alpha(175));
                    let center = rect.center();
                    // Overlay widgets must never reposition the parent cursor, which
                    // already sits below the scroll area for the count and footer.
                    let mut overlay = ui.new_child(UiBuilder::new().max_rect(rect));
                    overlay.set_clip_rect(rect.intersect(ui.clip_rect()));
                    overlay.put(
                        Rect::from_center_size(center - vec2(0., 12.), vec2(32., 32.)),
                        egui::Spinner::new().size(32.).color(color(t.accent)),
                    );
                    overlay.put(
                        Rect::from_min_max(
                            pos2(rect.left() + 16., center.y + 12.),
                            rect.max - vec2(16., 8.),
                        ),
                        Label::new(
                            RichText::new(&self.plugin_work.status)
                                .size(15.)
                                .color(Color32::WHITE),
                        )
                        .wrap(),
                    );
                    ctx.request_repaint();
                }
                ui.label(
                    RichText::new(format!("Showing {shown} of {} plugins", self.plugins.len()))
                        .size(14.)
                        .color(color(t.muted)),
                );
                if self.plugin_work.rx.is_some() {
                    if self.plugin_work.mutating {
                        indeterminate_progress(ui, width, 5., t.accent, t.border);
                    }
                } else if !self.plugin_work.status.is_empty() {
                    ui.label(
                        RichText::new(&self.plugin_work.status)
                            .size(14.)
                            .color(color(t.muted)),
                    );
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Updates shown here support Unreal Engine {}.",
                            engine.version
                        ))
                        .color(color(t.muted)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close |= ui
                            .button("Done")
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked();
                    });
                });
            });
        if next != index {
            self.show_plugins(next);
            self.check_plugins(ctx.clone());
        }
        if close || (response.should_close() && self.plugin_work.confirm.is_none()) {
            self.plugin_engine = None;
        }
    }
}

fn plugin_cell(ui: &mut Ui, rect: Rect, text: RichText, tooltip: &str) {
    let mut cell_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    cell_ui.set_width(rect.width());
    cell_ui
        .add(Label::new(text).truncate())
        .on_hover_text(tooltip);
}
