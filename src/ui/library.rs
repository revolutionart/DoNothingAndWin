use super::*;
use engine_workspace::fab::{self, Item};
enum Event {
    Status(String),
    Loaded(Result<Vec<Item>, String>),
    Installed(Result<String, String>),
}
#[derive(Default)]
pub(super) struct Library {
    account: String,
    items: Vec<Item>,
    rx: Option<Receiver<Event>>,
    loading: bool,
    installing: bool,
    install_item: String,
    install_engine: String,
    attempted: bool,
    status: String,
    search: String,
    pub filter: usize,
    sort_desc: bool,
    list_view: bool,
    category: usize,
    sync_error: bool,
    pub footer: String,
    updates: std::collections::HashMap<String, Vec<(String, String)>>,
    update_rx: Option<Receiver<std::collections::HashMap<String, Vec<(String, String)>>>>,
    create_name: String,
    create_folder: String,
    selected: Option<Item>,
    target: usize,
    project_search: String,
    show_all_projects: bool,
    asset_version: String,
    images: Vec<(String, Receiver<Option<ColorImage>>)>,
    image_attempts: std::collections::HashSet<String>,
}
impl Workspace {
    pub(super) fn has_plugin_updates(&self) -> bool {
        self.engines.iter().any(|engine| {
            self.plugin_work
                .update_engines
                .get(&engine.path)
                .copied()
                .unwrap_or_else(|| {
                    self.library
                        .updates
                        .values()
                        .any(|entries| entries.iter().any(|(path, _)| path == &engine.path))
                })
        })
    }

    fn queue_library_thumbnail(&mut self, key: String, url: String, ctx: &Context) {
        if self.library.images.len() >= 8
            || self.textures.contains_key(&key)
            || !self.library.image_attempts.insert(key.clone())
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.library.images.push((key, rx));
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let image = fab::thumbnail_bytes(&url)
                .ok()
                .and_then(|raw| image::load_from_memory(&raw).ok())
                .map(|image| {
                    let image = image.thumbnail(480, 300).to_rgba8();
                    ColorImage::from_rgba_unmultiplied(
                        [image.width() as usize, image.height() as usize],
                        image.as_raw(),
                    )
                });
            let _ = tx.send(image);
            ctx.request_repaint();
        });
    }
    pub(super) fn library_transfer_status(&self) -> Option<&str> {
        self.library
            .installing
            .then_some(self.library.status.as_str())
    }
    fn refresh_library(&mut self, ctx: &Context) {
        if self.library.rx.is_some() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        self.library.attempted = true;
        self.library.loading = true;
        self.library.status = if self.library.items.is_empty() {
            "Loading your Fab library..."
        } else {
            "Syncing Fab library in the background…"
        }
        .into();
        let (tx, rx) = mpsc::channel();
        self.library.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = fab::library(&session, |n| {
                let _ = tx.send(Event::Status(format!("Loaded {n} library items...")));
                ctx.request_repaint();
            });
            if let Ok(items) = &result {
                let _ = fab::save_library_cache(&session.account_id, items);
            }
            let _ = tx.send(Event::Loaded(result));
            ctx.request_repaint();
        });
    }
    pub(super) fn poll_library(&mut self, ctx: &Context) {
        let account = self
            .session
            .as_ref()
            .map(|s| s.account_id.clone())
            .unwrap_or_default();
        if account != self.library.account {
            self.library = Library {
                items: fab::load_library_cache(&account),
                account,
                ..Default::default()
            };
            self.textures.retain(|key, _| !key.starts_with("fab:"));
            let mut cached = self.library.items.clone();
            cached.sort_by_key(|item| item.title.to_lowercase());
            for item in cached.iter().take(8) {
                if let Some(url) = item.thumbnail().filter(|url| fab::thumbnail_is_cached(url)) {
                    self.queue_library_thumbnail(format!("fab:{}", item.key()), url, ctx);
                }
            }
        }
        while let Some(event) = self.library.rx.as_ref().and_then(|r| r.try_recv().ok()) {
            match event {
                Event::Status(s) => self.library.status = s,
                Event::Loaded(result) => {
                    self.library.loading = false;
                    self.library.rx = None;
                    match result {
                        Ok(items) => {
                            self.library.status =
                                format!("{} items from your Fab account", items.len());
                            self.library.items = items;
                            self.library.sync_error = false;
                            let session = self.session.clone().unwrap();
                            let engines = self.engines.clone();
                            let items = self.library.items.clone();
                            let (tx, rx) = mpsc::channel();
                            self.library.update_rx = Some(rx);
                            self.library.updates.clear();
                            let ctx = ctx.clone();
                            std::thread::spawn(move || {
                                let mut updates = std::collections::HashMap::<
                                    String,
                                    Vec<(String, String)>,
                                >::new();
                                for engine in engines {
                                    if let Ok(checked) =
                                        engine_workspace::plugins::check(&engine, &session)
                                    {
                                        for entry in
                                            checked.into_iter().filter(|p| p.update.is_some())
                                        {
                                            if let Some(item) = items.iter().find(|i| {
                                                entry.plugin.catalog_id.as_deref()
                                                    == Some(i.asset_id.as_str())
                                            }) {
                                                updates.entry(item.key()).or_default().push((
                                                    engine.path.clone(),
                                                    format!(
                                                        "Unreal Engine {}: {}",
                                                        engine.version, entry.status
                                                    ),
                                                ));
                                            }
                                        }
                                    }
                                }
                                for (key, message) in fab::project_updates(&session, &items) {
                                    updates
                                        .entry(key)
                                        .or_default()
                                        .push((String::new(), message));
                                }
                                let _ = tx.send(updates);
                                ctx.request_repaint();
                            });
                        }
                        Err(e) => {
                            self.library.sync_error = true;
                            self.library.status =
                                format!("Sync failed: {e}. Showing saved library.");
                        }
                    }
                }
                Event::Installed(result) => {
                    if let Ok(message) = &result {
                        if let Some(path) = message.strip_prefix("Created project: ") {
                            if let Ok(project) = platform::import_project(Path::new(path)) {
                                if !self.projects.iter().any(|p| p.path == project.path) {
                                    self.prefs.manual_projects.push(project.path.clone());
                                    self.projects.push(project);
                                    self.load_assets(ctx);
                                    self.persist();
                                }
                            }
                        }
                    }
                    self.record_activity(
                        self.library.install_item.clone(),
                        self.library.install_engine.clone(),
                        &result,
                    );
                    self.library.installing = false;
                    self.library.rx = None;
                    self.library.status =
                        result.unwrap_or_else(|e| format!("Installation failed: {e}"));
                    self.plugin_counts.clear();
                    self.workspace_cache.invalidate();
                    self.notify(self.library.status.clone());
                }
            }
        }
        if let Some(result) = self
            .library
            .update_rx
            .as_ref()
            .and_then(|r| r.try_recv().ok())
        {
            // A full library check supersedes earlier dialog checks.
            self.plugin_work.update_engines.clear();
            self.library.updates = result;
            self.library.update_rx = None;
        }
        let mut done = Vec::new();
        for (i, (key, rx)) in self.library.images.iter().enumerate() {
            if let Ok(data) = rx.try_recv() {
                if let Some(image) = data {
                    self.textures.insert(
                        key.clone(),
                        ctx.load_texture(key, image, TextureOptions::LINEAR),
                    );
                }
                done.push(i);
            }
        }
        for i in done.into_iter().rev() {
            self.library.images.remove(i);
        }
        if self.library.rx.is_some() || !self.library.images.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    pub(super) fn real_library_page(&mut self, ui: &mut Ui, c: &Canvas) {
        let narrow = c.width < 1120.;
        let header_y = if narrow { 108. } else { 38. };
        if c.button(
            ui,
            "sync-library",
            button_slot([c.width - 350., header_y, 170., 38.]),
            "Sync library",
            Some(Icon::Reset),
            false,
        ) && !self.library.installing
        {
            self.refresh_library(ui.ctx());
        }
        let link = c.rect([c.width - 165., header_y, 140., 38.]);
        ui.put(
            link,
            egui::Hyperlink::from_label_and_url("My Fab library ↗", "https://www.fab.com/library"),
        );
        let top = if narrow { 160. } else { 108. };
        c.line(
            ui,
            [271., top - 6.],
            [c.width - 26., top - 6.],
            self.theme.border,
        );
        if self.session.is_none() {
            ui.put(
                c.rect([271., top + 20., 500., 40.]),
                Label::new("Connect your Epic account in Settings to load your Fab library."),
            );
            return;
        }
        if !self.library.attempted {
            self.refresh_library(ui.ctx());
        }
        let theme = self.theme.clone();
        let mut choose = None;
        let mut update_engine = None;
        let rect = c.rect([271., top, c.width - 297., c.height - top - 72.]);
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.style_mut().override_font_id = Some(FontId::proportional(16.));
            ui.spacing_mut().button_padding = vec2(14., 8.);
            ui.spacing_mut().item_spacing = vec2(16., 12.);
            ui.spacing_mut().interact_size.y = 36.;
            let scroll = &mut ui.style_mut().spacing.scroll;
            scroll.floating = false;
            scroll.bar_width = 6.;
            scroll.floating_width = 6.;
            scroll.bar_inner_margin = 2.;
            scroll.bar_outer_margin = 2.;
            let old = (
                self.library.filter,
                self.library.search.clone(),
                self.library.category,
                self.library.sort_desc,
            );
            ui.horizontal_wrapped(|ui| {
                ui.style_mut().override_font_id = Some(FontId::proportional(14. * c.scale()));
                ui.spacing_mut().button_padding = vec2(10., 5.) * c.scale();
                ui.spacing_mut().item_spacing = vec2(8., 8.) * c.scale();
                ui.spacing_mut().interact_size.y = 32. * c.scale();
                for (index, name) in [(0, "All"), (1, "Assets"), (3, "Projects"), (2, "Plugins")] {
                    let count = self
                        .library
                        .items
                        .iter()
                        .filter(|i| library_kind(i, index))
                        .count();
                    let selected = self.library.filter == index;
                    let response = ui
                        .scope(|ui| {
                            ui.visuals_mut().widgets.inactive.fg_stroke.color =
                                color(if selected { theme.accent } else { theme.muted });
                            ui.add(
                                Button::new(RichText::new(format!("{name} ({count})")).color(
                                    color(if selected { theme.accent } else { theme.muted }),
                                ))
                                .frame(false)
                                .min_size(vec2(96., 32.) * c.scale()),
                            )
                        })
                        .inner;
                    if selected {
                        ui.painter().line_segment(
                            [response.rect.left_bottom(), response.rect.right_bottom()],
                            Stroke::new(2., color(theme.accent)),
                        );
                    }
                    if response
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.library.filter = index;
                    }
                }
                if rect.width() < 1200. {
                    ui.end_row();
                } else {
                    ui.add_space((rect.right() - ui.cursor().left() - 624. * c.scale()).max(0.));
                }
                search_field(
                    ui,
                    &mut self.library.search,
                    "Search library...",
                    258. * c.scale(),
                    &theme,
                    c.scale(),
                );
                ComboBox::from_id_salt("library-category")
                    .width(130. * c.scale())
                    .selected_text(
                        ["Category", "Asset packs", "Plugins", "Projects"][self.library.category],
                    )
                    .show_ui(ui, |ui| {
                        for (i, label) in ["All categories", "Asset packs", "Plugins", "Projects"]
                            .iter()
                            .enumerate()
                        {
                            ui.selectable_value(&mut self.library.category, i, *label)
                                .on_hover_cursor(CursorIcon::PointingHand);
                        }
                    })
                    .response
                    .on_hover_cursor(CursorIcon::PointingHand);
                ComboBox::from_id_salt("library-sort")
                    .width(132. * c.scale())
                    .selected_text(if self.library.sort_desc {
                        "Name Z – A"
                    } else {
                        "Name A – Z"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.library.sort_desc, false, "Name A – Z")
                            .on_hover_cursor(CursorIcon::PointingHand);
                        ui.selectable_value(&mut self.library.sort_desc, true, "Name Z – A")
                            .on_hover_cursor(CursorIcon::PointingHand);
                    })
                    .response
                    .on_hover_cursor(CursorIcon::PointingHand);
                for (list, icon) in [(false, Icon::Grid), (true, Icon::List)] {
                    let (r, response) =
                        ui.allocate_exact_size(vec2(32., 32.) * c.scale(), Sense::click());
                    ui.painter().rect_filled(
                        r,
                        5.,
                        color(if self.library.list_view == list {
                            theme.accent
                        } else if response.hovered() {
                            SIDEBAR_HIGHLIGHT
                        } else {
                            theme.card
                        }),
                    );
                    let local = Canvas {
                        origin: r.min,
                        width: 32.,
                        height: 32.,
                        sx: c.scale(),
                        sy: c.scale(),
                        theme: theme.clone(),
                    };
                    local.icon(
                        ui,
                        icon,
                        8.,
                        8.,
                        16.,
                        if self.library.list_view == list {
                            theme.on_accent
                        } else if response.hovered() {
                            SIDEBAR_ON_HIGHLIGHT
                        } else {
                            theme.muted
                        },
                    );
                    ui.painter().rect_stroke(
                        r,
                        BUTTON_RADIUS,
                        Stroke::new(1., color(theme.border)),
                        StrokeKind::Inside,
                    );
                    if response
                        .on_hover_text(if list { "List view" } else { "Grid view" })
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.library.list_view = list;
                    }
                }
            });
            if old
                != (
                    self.library.filter,
                    self.library.search.clone(),
                    self.library.category,
                    self.library.sort_desc,
                )
            {
                // Scroll identity changes with the filters below.
            }
            if self.library.loading
                || self.library.installing
                || self.library.update_rx.is_some()
                || self.library.sync_error
            {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().color(if self.theme.dark {
                        Color32::WHITE
                    } else {
                        color(self.theme.text)
                    }));
                    ui.label(
                        if self.library.loading
                            || self.library.installing
                            || self.library.sync_error
                        {
                            self.library.status.as_str()
                        } else {
                            "Checking installed content for updates..."
                        },
                    );
                });
            }
            let query = self.library.search.to_lowercase();
            let mut items: Vec<_> = self
                .library
                .items
                .iter()
                .filter(|i| {
                    i.title.to_lowercase().contains(&query)
                        && library_kind(i, self.library.filter)
                        && library_kind(i, self.library.category)
                })
                .cloned()
                .collect();
            items.sort_by_key(|i| i.title.to_lowercase());
            if self.library.sort_desc {
                items.reverse();
            }
            self.library.footer = format!("{} items", items.len());
            let columns = ((ui.available_width() + 16.) / 326.).floor().max(1.) as usize;
            let list = self.library.list_view;
            ScrollArea::vertical()
                .id_salt((
                    "fab-owned-items",
                    self.library.filter,
                    self.library.category,
                    self.library.search.clone(),
                    self.library.sort_desc,
                    self.library.list_view,
                ))
                .max_height(ui.available_height())
                .auto_shrink([false, false])
                .show_rows(
                    ui,
                    if list { 96. } else { 234. },
                    items.len().div_ceil(if list { 1 } else { columns }),
                    |ui, range| {
                        for row in items
                            .chunks(if list { 1 } else { columns })
                            .skip(range.start)
                            .take(range.len())
                        {
                            ui.horizontal(|ui| {
                                for item in row {
                                    let key = format!("fab:{}", item.key());
                                    if let Some(url) = item.thumbnail() {
                                        self.queue_library_thumbnail(key.clone(), url, ui.ctx());
                                    }
                                    let width = if list {
                                        ui.available_width()
                                    } else {
                                        310_f32.min(ui.available_width())
                                    };
                                    let height = if list { 92. } else { 230. };
                                    let (r, _) =
                                        ui.allocate_exact_size(vec2(width, height), Sense::hover());
                                    let mut card_ui = ui.new_child(UiBuilder::new().max_rect(r));
                                    let ui = &mut card_ui;
                                    ui.painter().rect_filled(r, 6., color(theme.card));
                                    ui.painter().rect_stroke(
                                        r,
                                        6.,
                                        Stroke::new(1., color(theme.border)),
                                        StrokeKind::Inside,
                                    );
                                    let picture = if list {
                                        Rect::from_min_size(r.min + vec2(8., 8.), vec2(116., 76.))
                                    } else {
                                        Rect::from_min_size(
                                            r.min + vec2(1., 1.),
                                            vec2(width - 2., 140.),
                                        )
                                    };
                                    if let Some(tex) = self.textures.get(&key) {
                                        let size = tex.size_vec2();
                                        let ratio = (picture.width() / picture.height())
                                            / (size.x / size.y);
                                        let uv = if ratio < 1. {
                                            Rect::from_min_max(
                                                pos2((1. - ratio) / 2., 0.),
                                                pos2((1. + ratio) / 2., 1.),
                                            )
                                        } else {
                                            Rect::from_min_max(
                                                pos2(0., (1. - 1. / ratio) / 2.),
                                                pos2(1., (1. + 1. / ratio) / 2.),
                                            )
                                        };
                                        ui.painter().image(tex.id(), picture, uv, Color32::WHITE);
                                    } else {
                                        ui.painter().rect_filled(
                                            picture,
                                            5.,
                                            color(theme.selection),
                                        );
                                    }
                                    let title = if list {
                                        Rect::from_min_size(
                                            r.min + vec2(140., 12.),
                                            vec2((width - 390.).max(110.), 28.),
                                        )
                                    } else {
                                        Rect::from_min_size(
                                            r.min + vec2(12., 149.),
                                            vec2(width - 24., 25.),
                                        )
                                    };
                                    let mut job = egui::text::LayoutJob::simple_singleline(
                                        item.title.clone(),
                                        FontId::proportional(16.),
                                        color(theme.text),
                                    );
                                    job.wrap.max_width = title.width();
                                    job.wrap.max_rows = 1;
                                    job.wrap.break_anywhere = true;
                                    let galley = ui.painter().layout_job(job);
                                    ui.painter().galley(
                                        pos2(title.left(), title.center().y - galley.size().y / 2.),
                                        galley,
                                        color(theme.text),
                                    );
                                    ui.interact(
                                        title,
                                        ui.id().with((&key, "title")),
                                        Sense::hover(),
                                    )
                                    .on_hover_text(&item.title);
                                    let y = if list { r.center().y } else { r.bottom() - 28. };
                                    let badge = Rect::from_min_size(
                                        pos2(
                                            if list {
                                                r.left() + 140.
                                            } else {
                                                r.left() + 12.
                                            },
                                            if list { r.bottom() - 36. } else { y - 16. },
                                        ),
                                        vec2(86., 30.),
                                    );
                                    ui.painter().rect_stroke(
                                        badge,
                                        5.,
                                        Stroke::new(1., color(theme.border)),
                                        StrokeKind::Inside,
                                    );
                                    let kind = if item.plugin() {
                                        "Plugin"
                                    } else if item.distribution_method == "COMPLETE_PROJECT" {
                                        "Project"
                                    } else {
                                        "Asset"
                                    };
                                    library_type_icon(
                                        ui,
                                        badge.min + vec2(10., 7.),
                                        kind,
                                        color(theme.text),
                                    );
                                    ui.painter().text(
                                        badge.min + vec2(33., 15.),
                                        Align2::LEFT_CENTER,
                                        kind,
                                        FontId::proportional(14.),
                                        color(theme.text),
                                    );
                                    let updates = self.library.updates.get(&item.key());
                                    let has_update = updates.is_some_and(|u| !u.is_empty());
                                    let info = Rect::from_center_size(
                                        pos2(r.right() - 22., y),
                                        vec2(28., 32.),
                                    );
                                    let tint = if has_update {
                                        Color32::from_rgb(255, 153, 0)
                                    } else {
                                        color(theme.muted)
                                    };
                                    ui.painter().circle_stroke(
                                        info.center(),
                                        10.,
                                        Stroke::new(1.6, tint),
                                    );
                                    ui.painter().text(
                                        info.center(),
                                        Align2::CENTER_CENTER,
                                        "!",
                                        FontId::proportional(16.),
                                        tint,
                                    );
                                    let mut tooltip = format!(
                                        "Engine compatibility\n{}",
                                        if item.supported_branches().is_empty() {
                                            "No Windows engine versions supplied".into()
                                        } else {
                                            format!(
                                                "Unreal Engine {}",
                                                item.supported_branches().join(", ")
                                            )
                                        }
                                    );
                                    if let Some(updates) = updates {
                                        for (_, message) in updates {
                                            tooltip.push_str(&format!("\n{message}"));
                                        }
                                    }
                                    ui.interact(
                                        info,
                                        ui.id().with((&key, "compatibility")),
                                        Sense::click(),
                                    )
                                    .on_hover_text(tooltip);
                                    let action = Rect::from_min_size(
                                        pos2(r.right() - 190., y - 17.),
                                        vec2(146., 34.),
                                    );
                                    let response = ui
                                        .interact(
                                            action,
                                            ui.id().with((&key, "install")),
                                            Sense::click(),
                                        )
                                        .on_hover_cursor(CursorIcon::PointingHand);
                                    let fill = if response.hovered() {
                                        theme.accent_hover
                                    } else {
                                        theme.accent
                                    };
                                    ui.painter().rect_filled(action, 5., color(fill));
                                    let label = if has_update && item.plugin() {
                                        "Update"
                                    } else if item.plugin() {
                                        "Install to engine"
                                    } else if item.distribution_method == "COMPLETE_PROJECT" {
                                        "Create project"
                                    } else {
                                        "Add to project"
                                    };
                                    ui.painter().text(
                                        action.center(),
                                        Align2::CENTER_CENTER,
                                        label,
                                        FontId::proportional(14.),
                                        color(theme.on_accent),
                                    );
                                    if response
                                        .clone()
                                        .on_hover_cursor(CursorIcon::PointingHand)
                                        .clicked()
                                        && !self.library.installing
                                    {
                                        if has_update && item.plugin() {
                                            update_engine = updates
                                                .and_then(|u| u.first())
                                                .map(|u| u.0.clone());
                                        } else {
                                            choose = Some(item.clone());
                                        }
                                    }
                                }
                            });
                            ui.add_space(4.);
                        }
                        if items.is_empty() && !self.library.loading {
                            ui.label(if self.library.status.contains("items from") {
                                "No matching library items."
                            } else {
                                &self.library.status
                            });
                        }
                    },
                );
        });
        c.line(
            ui,
            [271., c.height - 57.],
            [c.width - 26., c.height - 57.],
            theme.border,
        );
        if let Some(path) = update_engine {
            if let Some(i) = self.engines.iter().position(|e| e.path == path) {
                self.show_plugins(i);
                self.check_plugins(ui.ctx().clone());
            }
        }
        if let Some(item) = choose {
            self.library.create_name = item
                .title
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(48)
                .collect();
            self.library.selected = Some(item);
            self.library.target = if self.library.selected.as_ref().is_some_and(|i| i.plugin()) {
                0
            } else {
                usize::MAX
            };
            self.library.project_search.clear();
            self.library.asset_version.clear();
            self.library.show_all_projects = false;
        }
        if self
            .library
            .selected
            .as_ref()
            .is_some_and(|i| i.distribution_method == "COMPLETE_PROJECT")
        {
            self.library_create_dialog(ui.ctx());
        } else {
            self.library_install_dialog(ui.ctx());
        }
    }
    fn library_install_dialog(&mut self, ctx: &Context) {
        let Some(item) = self.library.selected.clone() else {
            return;
        };
        let targets: Vec<(String, String, String)> = if item.plugin() {
            self.engines
                .iter()
                .filter(|e| {
                    item.compatible(&e.version).is_some()
                        && (self.demo || platform::engine_ready(e))
                })
                .map(|e| {
                    (
                        format!("Unreal Engine {}", e.version),
                        e.path.clone(),
                        e.version.clone(),
                    )
                })
                .collect()
        } else {
            self.projects
                .iter()
                .filter(|p| self.project_visible(&p.path))
                .filter_map(|p| {
                    let version = self
                        .engines
                        .iter()
                        .find(|e| e.association == p.engine)
                        .map(|e| e.version.clone())
                        .unwrap_or_else(|| p.engine.clone());

                    Some((
                        p.name.clone(),
                        Path::new(&p.path).parent()?.to_string_lossy().into_owned(),
                        version,
                    ))
                })
                .collect()
        };
        let mut open = true;
        let mut install = false;
        dialog::show(
            ctx,
            &self.theme.clone(),
            "fab-install",
            if item.plugin() {
                "Install to engine"
            } else {
                "Add to project"
            },
            if item.plugin() {
                "Choose an engine for this plugin."
            } else {
                "Select a project for this asset."
            },
            &mut open,
            |ui| {
                Frame::new()
                    .fill(color(self.theme.card))
                    .stroke(Stroke::new(1., color(self.theme.border)))
                    .corner_radius(5.)
                    .inner_margin(8)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let size = if item.plugin() {
                                vec2(88., 88.)
                            } else {
                                vec2(135., 70.)
                            };
                            let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                            if let Some(tex) = self.textures.get(&format!("fab:{}", item.key())) {
                                ui.painter().image(
                                    tex.id(),
                                    r,
                                    Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                                    Color32::WHITE,
                                );
                            } else {
                                ui.painter().rect_filled(r, 4., color(self.theme.selection));
                            }
                            ui.vertical(|ui| {
                                ui.set_width((ui.available_width() - 10.).max(100.));
                                ui.add(
                                    Label::new(
                                        RichText::new(&item.title)
                                            .strong()
                                            .color(ui.visuals().text_color())
                                            .size(18.),
                                    )
                                    .wrap(),
                                );
                                ui.label(if item.plugin() { "Plugin" } else { "Asset" });
                            });
                        });
                    });
                if item.plugin() {
                    ui.label("Engine version");
                    if targets.is_empty() {
                        ui.add_enabled(
                            false,
                            Button::new("No compatible engines installed")
                                .min_size(vec2(ui.available_width(), 44.)),
                        );
                        Frame::new().fill(color(self.theme.card)).stroke(Stroke::new(1.,color(self.theme.warning))).corner_radius(6.).inner_margin(16).show(ui,|ui|{
                            ui.set_width(ui.available_width());
                            ui.horizontal_top(|ui|{
                                let(r,_)=ui.allocate_exact_size(vec2(34.,34.),Sense::hover());
                                ui.painter().add(Shape::convex_polygon(vec![r.center_top(),r.left_bottom(),r.right_bottom()],color(self.theme.warning),Stroke::NONE));
                                ui.painter().text(r.center()+vec2(0.,5.),Align2::CENTER_CENTER,"!",FontId::proportional(24.),color(self.theme.card));
                                ui.vertical(|ui|{
                                    ui.label(RichText::new("A compatible engine is required").strong().color(ui.visuals().text_color()));
                                    ui.label("This plugin cannot be installed because no compatible engine is installed. Install a compatible engine, then try again.");
                                });
                            });
                        });
                        if ui
                            .link("+ Install an engine")
                            .clone()
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.page = Page::Engines;
                            self.library.selected = None;
                        }
                    } else {
                        self.library.target = self.library.target.min(targets.len() - 1);
                        let selector_open = ComboBox::is_open(
                            ui.ctx(),
                            ui.make_persistent_id("fab-install-target"),
                        );
                        let combo = ComboBox::from_id_salt("fab-install-target")
                            .width(ui.available_width())
                            .selected_text(&targets[self.library.target].0)
                            .show_ui(ui, |ui| {
                                for (i, (name, path, _)) in targets.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut self.library.target,
                                        i,
                                        format!("{name}     Installed"),
                                    )
                                    .on_hover_text(path);
                                }
                            });
                        combo
                            .response
                            .clone()
                            .on_hover_cursor(CursorIcon::PointingHand);
                        let r = combo.response.rect;
                        ui.painter().circle_filled(
                            pos2(r.right() - 124., r.center().y),
                            5.,
                            color(self.theme.success),
                        );
                        ui.painter().text(
                            pos2(r.right() - 112., r.center().y),
                            Align2::LEFT_CENTER,
                            "Installed",
                            FontId::proportional(14.),
                            if selector_open {
                                ui.visuals().widgets.open.text_color()
                            } else if combo.response.hovered()
                                || combo.response.is_pointer_button_down_on()
                            {
                                ui.style().interact(&combo.response).text_color()
                            } else {
                                color(self.theme.muted)
                            },
                        );
                        ui.label(
                            RichText::new("Only compatible installed engines are listed.")
                                .color(color(self.theme.muted)),
                        );
                        ui.label(format!(
                            "This plugin will be installed for {} only.",
                            targets[self.library.target].0
                        ));
                    }
                } else {
                    ui.horizontal(|ui| {
                        search_field(
                            ui,
                            &mut self.library.project_search,
                            "Search projects...",
                            (ui.available_width() - 190.).max(100.),
                            &self.theme,
                            1.,
                        );
                        ui.checkbox(
                            &mut self.library.show_all_projects,
                            RichText::new("Show all projects").color(color(self.theme.text)),
                        )
                        .on_hover_cursor(CursorIcon::PointingHand);
                    });
                    ui.label(
                        RichText::new(if self.library.show_all_projects {
                            "Select any project, then choose an alternative asset version if needed."
                        } else {
                            "Showing compatible projects."
                        })
                        .color(color(self.theme.muted)),
                    );
                    let visible: Vec<_> = targets
                        .iter()
                        .enumerate()
                        .filter(|(_, t)| {
                            t.0.to_lowercase()
                                .contains(&self.library.project_search.to_lowercase())
                                && (self.library.show_all_projects
                                    || item.compatible(&t.2).is_some())
                        })
                        .collect();
                    ScrollArea::vertical().id_salt("fab-project-picker").max_height((ctx.content_rect().height()-530.).clamp(120.,300.)).auto_shrink([false,true]).show(ui,|ui|{
                        let width=(ui.available_width()-24.)/3.;
                        for row in visible.chunks(3){ui.horizontal(|ui|{for &(index,(name,path,version)) in row {
                            let compatible=item.compatible(version).is_some();
                            let selectable = compatible || !item.import_branches(version).is_empty();
                            let(r,response)=ui.allocate_exact_size(vec2(width,150.),if selectable { Sense::click() } else { Sense::hover() });
                            let selected=selectable && self.library.target==index;
                            ui.painter().rect_filled(r,5.,color(self.theme.card));
                            ui.painter().rect_stroke(r,5.,Stroke::new(if selected{2.}else{1.},color(if selected{self.theme.accent}else{self.theme.border})),StrokeKind::Inside);
                            let picture=Rect::from_min_max(r.min+vec2(2.,2.),pos2(r.right()-2.,r.bottom()-32.));
                            let artwork=self.projects.iter().find(|p|Path::new(&p.path).parent().is_some_and(|d|d==Path::new(path))).and_then(|p|p.artwork.as_ref()).and_then(|a|self.textures.get(a));
                            if let Some(tex)=artwork{ui.painter().image(tex.id(),picture,Rect::from_min_max(Pos2::ZERO,pos2(1.,1.)),Color32::WHITE);}
                            let badge=Rect::from_min_size(picture.right_bottom()-vec2(52.,28.),vec2(48.,24.));ui.painter().rect_filled(badge,4.,color(self.theme.card));
                            ui.painter().text(badge.center(),Align2::CENTER_CENTER,EngineVersion::parse(version).map(|v|v.branch()).unwrap_or_else(||"Custom".into()),FontId::proportional(14.),color(self.theme.text));
                            let text=ui.painter().with_clip_rect(Rect::from_min_max(pos2(r.left()+8.,r.bottom()-30.),r.max-vec2(6.,2.)));
                            text.text(pos2(r.left()+8.,r.bottom()-16.),Align2::LEFT_CENTER,name,FontId::proportional(14.),color(self.theme.text));
                            if !selectable {
                                // Dim the entire card, including its artwork, without disabling hover explanations.
                                ui.painter().rect_filled(r,5.,color(self.theme.sidebar).gamma_multiply(0.65));
                                let reason = match item.supported_branches().first() {
                                    Some(minimum) => format!("This project uses Unreal Engine {version}. The oldest available asset version is {minimum}; newer assets cannot be added to an older engine project."),
                                    None => "This asset has no available Windows version for this project.".into(),
                                };
                                response.on_hover_cursor(CursorIcon::NotAllowed).on_hover_text(reason);
                            } else {
                                if response.clone().on_hover_cursor(CursorIcon::PointingHand).clicked() && self.library.target != index {self.library.target=index; self.library.asset_version.clear();}
                                response.on_hover_text(if compatible{path.clone()}else{format!("Choose an alternative asset version for this project ({version})")});
                            }
                        }});ui.add_space(8.);}
                        if visible.is_empty(){ui.label("No matching compatible projects. Import a project or enable Show all projects.");}
                    });
                    if let Some(target) = targets.get(self.library.target) {
                        if item.compatible(&target.2).is_none() {
                            let branches = item.import_branches(&target.2);
                            let message = if branches.is_empty() {
                                "No asset version is available for this project's engine or an older engine. Assets made for newer engines cannot be added to this project."
                            } else {
                                "This asset does not list support for the project's engine. Choose an older asset version below. Compatibility is not guaranteed."
                            };
                            ui.add(
                                Label::new(RichText::new(message).color(color(self.theme.danger)))
                                    .wrap(),
                            );
                            if !branches.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.label("Asset version");
                                    ComboBox::from_id_salt("fab-alternative-version")
                                        .selected_text(if self.library.asset_version.is_empty() {
                                            "Select version"
                                        } else {
                                            &self.library.asset_version
                                        })
                                        .show_ui(ui, |ui| {
                                            for branch in branches {
                                                ui.selectable_value(
                                                    &mut self.library.asset_version,
                                                    branch.clone(),
                                                    format!("Unreal Engine {branch}"),
                                                );
                                            }
                                        });
                                });
                            }
                        }
                    }
                    if self.library.target == usize::MAX {
                        ui.label("Choose a project to continue.");
                    }
                }
                let valid = targets.get(self.library.target).is_some_and(|t| {
                    item.import_version(&t.2, &self.library.asset_version)
                        .is_some()
                });
                install = dialog::footer(
                    ui,
                    &self.theme,
                    if item.plugin() {
                        "Install"
                    } else {
                        "Add to project"
                    },
                    valid && !self.demo && self.library.rx.is_none() && !self.transfer.busy(),
                    false,
                );
            },
        );
        if !open {
            self.library.selected = None;
        }
        if install {
            let (_, target, engine) = targets[self.library.target].clone();
            let Some((version, engine)) = item.import_version(&engine, &self.library.asset_version)
            else {
                return;
            };
            let Some(session) = self.session.clone() else {
                return;
            };
            let (tx, rx) = mpsc::channel();
            self.library.rx = Some(rx);
            self.library.installing = true;
            self.library.install_item = format!("Installation: {}", item.title);
            self.library.install_engine = engine.clone();
            self.library.selected = None;
            self.library.status = "Preparing download...".into();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = fab::install(
                    &session,
                    &item,
                    &version,
                    Path::new(&target),
                    &engine,
                    |s| {
                        let _ = tx.send(Event::Status(s));
                        ctx.request_repaint();
                    },
                );
                let _ = tx.send(Event::Installed(result));
                ctx.request_repaint();
            });
        }
    }
}

fn library_kind(item: &Item, filter: usize) -> bool {
    match filter {
        1 => item.distribution_method == "ASSET_PACK",
        2 => item.plugin(),
        3 => item.distribution_method == "COMPLETE_PROJECT",
        _ => true,
    }
}
fn library_type_icon(ui: &Ui, origin: Pos2, kind: &str, tint: Color32) {
    let p = ui.painter();
    let s = Stroke::new(1.4, tint);
    let line = |a: [f32; 2], b: [f32; 2]| {
        p.line_segment([origin + vec2(a[0], a[1]), origin + vec2(b[0], b[1])], s);
    };
    if kind == "Asset" {
        for (a, b) in [
            ([0., 4.], [7., 0.]),
            ([7., 0.], [14., 4.]),
            ([14., 4.], [14., 12.]),
            ([14., 12.], [7., 16.]),
            ([7., 16.], [0., 12.]),
            ([0., 12.], [0., 4.]),
            ([0., 4.], [7., 8.]),
            ([7., 8.], [14., 4.]),
            ([7., 8.], [7., 16.]),
        ] {
            line(a, b);
        }
    } else if kind == "Project" {
        for (a, b) in [
            ([0., 3.], [6., 3.]),
            ([6., 3.], [8., 6.]),
            ([8., 6.], [15., 6.]),
            ([15., 6.], [15., 15.]),
            ([15., 15.], [0., 15.]),
            ([0., 15.], [0., 3.]),
        ] {
            line(a, b);
        }
    } else {
        p.rect_stroke(
            Rect::from_min_size(origin + vec2(2., 5.), vec2(11., 10.)),
            1.,
            s,
            StrokeKind::Inside,
        );
        p.circle_stroke(origin + vec2(7.5, 3.), 2.5, s);
        p.circle_stroke(origin + vec2(15., 10.), 2.5, s);
    }
}
impl Workspace {
    fn library_create_dialog(&mut self, ctx: &Context) {
        let Some(item) = self.library.selected.clone() else {
            return;
        };
        let mut open = true;
        let mut create = false;
        dialog::show(
            ctx,
            &self.theme.clone(),
            "fab-create-project",
            "Create project",
            "Create a new project from this Fab template.",
            &mut open,
            |ui| {
                ui.label(
                    RichText::new(&item.title)
                        .strong()
                        .color(ui.visuals().text_color()),
                );
                ui.label("Project folder name");
                ui.add(
                    TextEdit::singleline(&mut self.library.create_name)
                        .vertical_align(Align::Center)
                        .margin(vec2(12., 8.)),
                );
                ui.label("Destination folder");
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [ui.available_width() - 115., 40.],
                        TextEdit::singleline(&mut self.library.create_folder)
                            .vertical_align(Align::Center)
                            .margin(vec2(12., 8.)),
                    );
                    if ui
                        .button("Browse")
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.library.create_folder = p.display().to_string();
                        }
                    }
                });
                ComboBox::from_id_salt("template-version")
                    .selected_text(if self.library.asset_version.is_empty() {
                        "Choose engine version"
                    } else {
                        &self.library.asset_version
                    })
                    .show_ui(ui, |ui| {
                        for branch in item.supported_branches() {
                            ui.selectable_value(
                                &mut self.library.asset_version,
                                branch.clone(),
                                format!("Unreal Engine {branch}"),
                            );
                        }
                    });
                ui.label("The template's original project and module names are preserved inside your new folder. Existing projects stay untouched.");
                let destination =
                    Path::new(&self.library.create_folder).join(&self.library.create_name);
                ui.label(format!("New project folder: {}", destination.display()));
                let valid = !self.library.create_name.is_empty()
                    && self.library.create_name.len() <= 64
                    && self
                        .library
                        .create_name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && Path::new(&self.library.create_folder).is_dir()
                    && !destination.exists()
                    && item.compatible(&self.library.asset_version).is_some();
                create = dialog::footer(
                    ui,
                    &self.theme,
                    "Create project",
                    valid && self.library.rx.is_none() && !self.transfer.busy(),
                    false,
                );
            },
        );
        if !open {
            self.library.selected = None;
        }
        if create {
            let Some(session) = self.session.clone() else {
                return;
            };
            let branch = self.library.asset_version.clone();
            let Some(version) = item.compatible(&branch) else {
                return;
            };
            let folder = self.library.create_folder.clone();
            let name = self.library.create_name.clone();
            self.library.selected = None;
            self.library.installing = true;
            self.library.install_item = format!("Project creation: {}", item.title);
            self.library.install_engine = branch.clone();
            let (tx, rx) = mpsc::channel();
            self.library.rx = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = fab::create_project(
                    &session,
                    &item,
                    &version,
                    Path::new(&folder),
                    &name,
                    &branch,
                    |s| {
                        let _ = tx.send(Event::Status(s));
                        ctx.request_repaint();
                    },
                );
                let _ = tx.send(Event::Installed(result));
                ctx.request_repaint();
            });
        }
    }
}
