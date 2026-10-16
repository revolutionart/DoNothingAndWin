use super::*;
use engine_workspace::plugins::{self, Checked, Plugin, Update};
use std::sync::{Arc, Mutex};
enum Event {
    CheckRow(String, Instant, Checked),
    Updated(String, Instant, Plugin),
    Checked(String, Instant, Result<Vec<Checked>, String>),
    Finished(String, Result<String, String>),
}
#[derive(Default)]
pub(super) struct PluginWork {
    rx: Option<Receiver<Event>>,
    pub mutating: bool,
    pub checked: Vec<Checked>,
    pub checks: std::collections::HashMap<String, (Instant, Vec<Checked>)>,
    pub status: String,
    check_done: usize,
    check_total: usize,
    pub progress: Option<Arc<Mutex<PluginProgress>>>,
    operation: String,
    activity: super::plugin_activity::Activity,
    pub icons_loaded: std::collections::HashSet<String>,
    confirm: Option<(Installation, Plugin)>,
}
#[derive(Clone, Default)]
pub(super) struct PluginProgress {
    pub title: String,
    pub icon_key: String,
    pub state: plugins::UpdateProgress,
}
impl PluginWork {
    // Explicit render-probe fixture. Never starts a transfer or changes installed files.
    pub(super) fn preview_activity(&mut self) {
        let now = Instant::now();
        let mut state = plugins::UpdateProgress {
            phase: "Preparing and verifying files",
            total: 8_139_000_000,
            package_bytes: Some(1_480_000_000),
            files_total: 10627,
            ..Default::default()
        };
        let title = "Preview: PCGEx 0.75.6 → 0.76.1 • UE 5.6.1";
        self.activity = Default::default();
        for i in 0..=120 {
            state.downloaded += if i < 90 { 12_000_000 } else { 0 };
            state.read += 2_000_000;
            state.written += 30_000_000 + (i % 13) * 1_000_000;
            self.activity
                .sample(title, &state, now - Duration::from_millis((120 - i) * 500));
        }
        state.prepared = 3_900_000_000;
        state.files_done = 3179;
        self.status = state.phase.into();
        self.mutating = true;
        self.progress = Some(Arc::new(Mutex::new(PluginProgress {
            title: title.into(),
            icon_key: String::new(),
            state,
        })));
    }
    pub fn snapshot(&self) -> Option<PluginProgress> {
        self.progress.as_ref().map(|p| p.lock().unwrap().clone())
    }
    pub fn fraction(&self) -> Option<f32> {
        self.snapshot().and_then(|p| p.state.fraction())
    }
    pub fn show_progress(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        textures: &HashMap<String, TextureHandle>,
        bordered: bool,
    ) {
        if let Some(p) = self.snapshot() {
            self.activity.show(ui, theme, &p, textures, bordered);
        } else {
            indeterminate_progress(ui, ui.available_width(), 8., theme.accent, theme.border);
        }
    }
}

impl Workspace {
    pub(super) fn check_plugins(&mut self, ctx: Context) {
        self.start_plugin_check(ctx, false);
    }
    fn start_plugin_check(&mut self, ctx: Context, force: bool) {
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
        self.plugin_work.check_done = 0;
        self.plugin_work.check_total = self.plugins.len();
        self.plugin_work.status = "Checking compatible plugin builds…".into();
        let started = Instant::now();
        std::thread::spawn(move || {
            let result = plugins::check_cached_with_progress(&engine, &session, force, |row| {
                let _ = tx.send(Event::CheckRow(engine.path.clone(), started, row.clone()));
                ctx.request_repaint();
            });
            let _ = tx.send(Event::Checked(engine.path, started, result));
            ctx.request_repaint();
        });
    }
    pub(super) fn poll_plugins(&mut self, ctx: &Context) {
        if self.plugin_work.mutating {
            if let Some(p) = self.plugin_work.snapshot() {
                self.plugin_work.status = p.state.phase.into();
                self.plugin_work
                    .activity
                    .sample(&p.title, &p.state, Instant::now());
            }
        }
        let event = self
            .plugin_work
            .rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if self.plugin_work.rx.is_some() && !crate::window_frame::is_backgrounded() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        if let Some(event) = event {
            if !matches!(&event, Event::Updated(..) | Event::CheckRow(..)) {
                self.plugin_work.rx = None;
                self.plugin_work.mutating = false;
                self.plugin_work.progress = None;
            }
            match event {
                Event::CheckRow(path, started, row) => {
                    self.record_plugin_check_row(path, started, row);
                    self.plugin_work.check_done += 1;
                    ctx.request_repaint();
                }
                Event::Updated(path, completed, plugin) => {
                    self.record_completed_plugin_update(path.clone(), completed, plugin.clone());
                    if self
                        .plugin_engine
                        .is_some_and(|i| self.engines[i].path == path)
                    {
                        if let Some(local) = self
                            .plugins
                            .iter_mut()
                            .find(|p| p.descriptor == plugin.descriptor)
                        {
                            *local = plugin;
                        }
                        self.plugin_work.checked = self.plugin_work.checks[&path].1.clone();
                    }
                    ctx.request_repaint();
                }
                Event::Checked(path, started, result) => {
                    if let Ok(items) = &result {
                        self.record_plugin_check(path.clone(), started, items.clone());
                    }
                    if self
                        .plugin_engine
                        .is_some_and(|i| self.engines[i].path == path)
                    {
                        match result {
                            Ok(items) => {
                                self.plugin_work.status = plugins::check_summary(&items);
                                self.plugin_work.checked = items;
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
                    self.record_activity(self.plugin_work.operation.clone(), version, &result);
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
        let count = items.len();
        self.plugin_work.operation = format!(
            "Update {}",
            items
                .iter()
                .map(|(p, _)| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.plugin_work.status = "Preparing plugin update…".into();
        let progress = Arc::new(Mutex::new(PluginProgress {
            title: format!("{} • UE {}", self.plugin_work.operation, engine.version),
            icon_key: String::new(),
            state: plugins::UpdateProgress {
                phase: "Preparing plugin update…",
                ..Default::default()
            },
        }));
        self.plugin_work.activity = Default::default();
        self.plugin_work.progress = Some(progress.clone());
        std::thread::spawn(move || {
            let result = (|| {
                let mut messages = vec![];
                for (index, (p, u)) in items.into_iter().enumerate() {
                    let title = format!(
                        "{} {} → {} • UE {} • Plugin {} of {}",
                        p.name,
                        p.version,
                        u.version,
                        engine.version,
                        index + 1,
                        count
                    );
                    match plugins::update_plugin_with_progress(&engine, &p, &u, |state| {
                        *progress.lock().unwrap() = PluginProgress {
                            title: title.clone(),
                            icon_key: p.descriptor.to_string_lossy().into_owned(),
                            state,
                        };
                        crate::window_frame::progress_repaint(&ctx);
                    }) {
                        Ok(m) => {
                            let mut installed = p.clone();
                            installed.version = u.version.clone();
                            installed.number = u.number;
                            let _ = tx.send(Event::Updated(
                                engine.path.clone(),
                                Instant::now(),
                                installed,
                            ));
                            ctx.request_repaint();
                            messages.push(m);
                        }
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
            self.plugin_work.operation = self.plugin_work.status.clone();
            self.plugin_work.progress = None;
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
        let busy = self.plugin_work.mutating || self.transfer.busy();
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
                ui.spacing_mut().item_spacing =
                    vec2(12., if self.plugin_work.mutating { 8. } else { 12. });
                ui.spacing_mut().button_padding = vec2(12., 5.);
                ui.spacing_mut().interact_size.y = COMPACT_BUTTON_HEIGHT;
                ui.style_mut().override_font_id = Some(FontId::proportional(16.));
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Engine plugins")
                            .size(27.)
                            .strong()
                            .color(ui.visuals().text_color()),
                    );
                    ui.add_enabled_ui(!self.plugin_work.mutating, |ui| {
                        AppComboBox::new(&self.theme, "plugin-dialog-engine")
                            .selected_text(format!("UE {}", engine.version))
                            .width(130.)
                            .show_ui(ui, |ui| {
                                for (i, e) in self.engines.iter().enumerate() {
                                    ui.selectable_value(&mut next, i, format!("UE {}", e.version));
                                }
                            });
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let close_button = ui
                            .add(plugin_button("").min_size(Vec2::splat(COMPACT_BUTTON_HEIGHT)))
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .on_hover_text("Close");
                        close_button.widget_info(|| {
                            WidgetInfo::labeled(WidgetType::Button, close_button.enabled(), "Close")
                        });
                        let icon =
                            Rect::from_center_size(close_button.rect.center(), Vec2::splat(14.));
                        let stroke =
                            Stroke::new(2., ui.style().interact(&close_button).fg_stroke.color);
                        ui.painter()
                            .line_segment([icon.left_top(), icon.right_bottom()], stroke);
                        ui.painter()
                            .line_segment([icon.right_top(), icon.left_bottom()], stroke);
                        close |= close_button.clicked();
                        if !updates.is_empty()
                            && ui
                                .add_enabled(
                                    !busy && self.plugin_work.rx.is_none(),
                                    primary_button(format!("Update all ({})", updates.len()), &t),
                                )
                                .on_hover_cursor(CursorIcon::PointingHand)
                                .clicked()
                        {
                            self.run_plugin_updates(updates.clone(), ctx.clone());
                        }
                    });
                });
                ui.label(
                    RichText::new(format!(
                        "Manage plugins installed for Unreal Engine {}.",
                        engine.version
                    ))
                    .color(color(t.muted)),
                );
                ui.add_space(4.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.;
                    for (flag, label, tab_width) in [
                        (false, format!("Installed ({})", self.plugins.len()), 122.),
                        (
                            true,
                            format!(
                                "Updates ({})",
                                self.plugin_work
                                    .checked
                                    .iter()
                                    .filter(|row| row.has_update())
                                    .count()
                            ),
                            112.,
                        ),
                    ] {
                        let selected = self.plugin_updates_only == flag;
                        let (rect, response) = ui.allocate_exact_size(
                            vec2(tab_width, COMPACT_BUTTON_HEIGHT),
                            Sense::click(),
                        );
                        response.widget_info(|| {
                            WidgetInfo::selected(
                                WidgetType::SelectableLabel,
                                true,
                                selected,
                                &label,
                            )
                        });
                        ui.painter().text(
                            rect.center() - vec2(0., 2.),
                            Align2::CENTER_CENTER,
                            &label,
                            FontId::proportional(COMPACT_BUTTON_FONT_SIZE),
                            color(if selected || response.hovered() {
                                t.text
                            } else {
                                t.muted
                            }),
                        );
                        if selected {
                            ui.painter().rect_filled(
                                Rect::from_min_max(
                                    pos2(rect.left(), rect.bottom() - 3.),
                                    rect.right_bottom(),
                                ),
                                0.,
                                color(t.accent),
                            );
                        }
                        if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                            self.plugin_updates_only = flag;
                        }
                    }
                    search_field(
                        ui,
                        &mut self.plugin_search,
                        "Search plugins...",
                        (width - 606.).max(150.),
                        &t,
                        1.,
                    );
                    if ui
                        .add_enabled(
                            !busy && self.plugin_work.rx.is_none(),
                            plugin_button("↻  Check updates")
                                .min_size(vec2(150., COMPACT_BUTTON_HEIGHT)),
                        )
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.start_plugin_check(ctx.clone(), true);
                    }
                    if ui
                        .add(
                            primary_button("+  Install from Fab", &t)
                                .min_size(vec2(174., COMPACT_BUTTON_HEIGHT)),
                        )
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.page = Page::Library;
                        self.library.filter = 2;
                        close = true;
                    }
                });
                ui.separator();
                let table_width = width - 12.;
                let columns = [
                    0.,
                    table_width * 0.32,
                    table_width * 0.43,
                    table_width * 0.54,
                    table_width - 248.,
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
                    .max_height(
                        (ctx.content_rect().height()
                            - if self.plugin_work.mutating {
                                570.
                            } else {
                                520.
                            })
                        .clamp(if self.plugin_work.mutating { 58. } else { 120. }, 400.),
                    )
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.;
                        // Keep room for the spinner and wrapped status even with no rows.
                        ui.set_min_height(if self.plugin_work.mutating { 58. } else { 120. });
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
                                || (self.plugin_updates_only
                                    && !checked.as_ref().is_some_and(|c| c.has_update()))
                            {
                                continue;
                            }
                            shown += 1;
                            let (row, _) =
                                ui.allocate_exact_size(vec2(table_width, 64.), Sense::hover());
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
                            let has_update = checked.as_ref().is_some_and(|c| c.has_update());
                            if has_update {
                                let tint = color(t.warning).gamma_multiply(if hovered {
                                    0.13
                                } else {
                                    0.07
                                });
                                ui.painter().rect_filled(row, 4., tint);
                                ui.painter().rect_stroke(
                                    row,
                                    4.,
                                    Stroke::new(1., color(t.warning).gamma_multiply(0.22)),
                                    StrokeKind::Inside,
                                );
                                ui.painter().rect_filled(
                                    Rect::from_min_size(row.min, vec2(3., row.height())),
                                    2.,
                                    color(t.warning),
                                );
                            } else if hovered {
                                ui.painter().rect_filled(row, 0., table_row_hover_color(&t));
                            }
                            let image_rect =
                                Rect::from_min_size(row.min + vec2(12., 12.5), vec2(39., 39.));
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
                                cell(53., columns[1]),
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
                            let available = checked
                                .as_ref()
                                .and_then(|c| c.available_version.as_deref())
                                .unwrap_or("-");
                            plugin_cell(
                                ui,
                                cell(columns[2], columns[3]),
                                RichText::new(available),
                                available,
                            );
                            let detail = checked
                                .as_ref()
                                .map(|i| i.status_detail())
                                .unwrap_or("Not checked");
                            let status = checked
                                .as_ref()
                                .map(|item| item.status_label())
                                .unwrap_or("Not checked");
                            let col = if checked.as_ref().is_some_and(|c| c.has_update()) {
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
                            let folder_rect = Rect::from_min_size(
                                pos2(
                                    action.right() - COMPACT_BUTTON_HEIGHT,
                                    row.center().y - COMPACT_BUTTON_HEIGHT / 2.,
                                ),
                                Vec2::splat(COMPACT_BUTTON_HEIGHT),
                            );
                            let button_width = (action.width() - COMPACT_BUTTON_HEIGHT - 16.) / 2.;
                            let update_rect = Rect::from_min_size(
                                pos2(action.left(), row.center().y - COMPACT_BUTTON_HEIGHT / 2.),
                                vec2(button_width, COMPACT_BUTTON_HEIGHT),
                            );
                            let remove_rect = update_rect.translate(vec2(button_width + 8., 0.));
                            if let Some(update) = update {
                                let response = ui
                                    .add_enabled_ui(!busy && self.plugin_work.rx.is_none(), |ui| {
                                        ui.put(update_rect, primary_button("Update", &t))
                                    })
                                    .inner
                                    .on_hover_cursor(CursorIcon::PointingHand);
                                if response.clicked() {
                                    self.run_plugin_updates(vec![(p.clone(), update)], ctx.clone());
                                }
                            }
                            let folder = ui
                                .add_enabled_ui(!self.plugin_work.mutating && !self.demo, |ui| {
                                    ui.put(folder_rect, plugin_button(""))
                                })
                                .inner
                                .on_hover_cursor(CursorIcon::PointingHand);
                            folder.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    folder.enabled(),
                                    "Open installed plugin folder",
                                )
                            });
                            let icon = Canvas {
                                origin: folder_rect.min,
                                width: COMPACT_BUTTON_HEIGHT,
                                height: COMPACT_BUTTON_HEIGHT,
                                sx: 1.,
                                sy: 1.,
                                theme: t.clone(),
                            };
                            icon.icon(
                                ui,
                                Icon::Folder,
                                7.,
                                7.,
                                18.,
                                if folder.hovered() {
                                    SIDEBAR_ON_HIGHLIGHT
                                } else {
                                    t.muted
                                },
                            );
                            if folder
                                .on_hover_text("Open installed plugin folder")
                                .clicked()
                            {
                                if let Some(path) = p.descriptor.parent() {
                                    if let Err(error) =
                                        platform::open_external(&path.to_string_lossy())
                                    {
                                        self.notify(error);
                                    }
                                }
                            }
                            let remove = ui
                                .add_enabled_ui(!busy && self.plugin_work.rx.is_none(), |ui| {
                                    let widgets = &mut ui.visuals_mut().widgets;
                                    widgets.inactive.fg_stroke.color = color(t.danger);
                                    for widget in [&mut widgets.hovered, &mut widgets.active] {
                                        widget.weak_bg_fill = color(t.danger);
                                        widget.bg_fill = color(t.danger);
                                        widget.fg_stroke.color = Color32::WHITE;
                                    }
                                    ui.put(
                                        remove_rect,
                                        plugin_button("Remove")
                                            .stroke(Stroke::new(1., color(t.danger))),
                                    )
                                })
                                .inner
                                .on_hover_cursor(CursorIcon::PointingHand);
                            if remove.clicked() {
                                self.plugin_work.confirm = Some((engine.clone(), p.clone()));
                            }
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
                                    })
                                    .gamma_multiply(0.5),
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
                if self.plugin_work.mutating {
                    // egui returns the viewport before auto-shrink. Match the actual
                    // reserved height so the overlay cannot cover the status/footer.
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
                if self.plugin_work.rx.is_some() || self.plugin_work.mutating {
                    if self.plugin_work.mutating {
                        self.plugin_work
                            .show_progress(ui, &t, &self.textures, false);
                    } else {
                        ui.horizontal(|ui| {
                            ui.add(Spinner::new().size(16.).color(color(t.accent)));
                            ui.label(
                                RichText::new(format!(
                                    "Checking plugin updates: {} of {} checked…",
                                    self.plugin_work.check_done, self.plugin_work.check_total
                                ))
                                .size(14.)
                                .color(color(t.muted)),
                            );
                        });
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
                            .add(plugin_button("Done"))
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

// Secondary plugin actions use the same compact dimensions as primary actions.
fn plugin_button(label: impl Into<String>) -> buttons::SecondaryButton<'static> {
    buttons::secondary_button(
        RichText::new(label.into()).font(FontId::proportional(COMPACT_BUTTON_FONT_SIZE)),
    )
    .min_size(vec2(0., COMPACT_BUTTON_HEIGHT))
}
