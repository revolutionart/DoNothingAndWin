use super::*;
use engine_workspace::vault_cache::{self, Entry, Inventory};
use std::path::PathBuf;

enum Event {
    Progress(String),
    Finished(Result<Inventory, String>),
}
#[derive(Default)]
pub(super) struct Vault {
    root: String,
    inventory: Option<Inventory>,
    rx: Option<Receiver<Event>>,
    error: Option<String>,
    progress: String,
    search: String,
    sort_name: bool,
    confirm: Option<Entry>,
    remove_rx: Option<Receiver<Result<(), String>>>,
    visible: bool,
    chunk_info: bool,
}
fn size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1e9)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else if bytes >= 1000 {
        format!("{:.1} KB", bytes as f64 / 1000.)
    } else {
        format!("{bytes} B")
    }
}
fn dots(ui: &mut Ui, theme: &Theme) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(32., 32.), Sense::click());
    let hovered = response.hovered();
    ui.painter().rect_filled(
        rect,
        4.,
        color(if hovered {
            SIDEBAR_HIGHLIGHT
        } else {
            theme.card
        }),
    );
    ui.painter().rect_stroke(
        rect,
        4.,
        Stroke::new(1., color(theme.border)),
        StrokeKind::Inside,
    );
    downloads::draw_icon(
        ui,
        theme,
        Icon::Dots,
        Rect::from_center_size(rect.center(), vec2(16., 16.)),
        if hovered {
            SIDEBAR_ON_HIGHLIGHT
        } else {
            theme.muted
        },
    );
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("More options")
}
impl Workspace {
    pub(super) fn preview_vault_cache(&mut self) {
        self.demo = true;
        self.vault.root = self.prefs.fab_cache.clone();
        if std::env::args().any(|a| a == "--search-filled-preview") {
            self.vault.search = "PCG".into();
        }
        self.vault.inventory = Some(Inventory {
            entries: [
                ("Shared download chunks", 31_405_000_000u64),
                ("PCGEx", 497_000_000),
                ("City Sample", 105_400_000_000),
                ("Assets Cleaner", 24_500_000),
                ("Unknown package", 2_500_000_000),
            ]
            .into_iter()
            .map(|(name, bytes)| Entry {
                path: Path::new(&self.prefs.fab_cache).join(name),
                title: name.into(),
                bytes,
                identities: vec![],
                removable: true,
                incomplete: false,
            })
            .collect(),
            warnings: vec![],
        });
    }
    fn scan_vault_cache(&mut self, ctx: &Context) {
        if self.vault.rx.is_some() || self.vault.remove_rx.is_some() {
            return;
        }
        self.vault.root = self.prefs.fab_cache.clone();
        self.vault.error = None;
        self.vault.progress = "Reading cache folders…".into();
        let root = PathBuf::from(&self.vault.root);
        let (tx, rx) = mpsc::channel();
        self.vault.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut last = Instant::now();
            let result = vault_cache::scan(&root, |path| {
                if last.elapsed() >= Duration::from_millis(150) {
                    let _ = tx.send(Event::Progress(path.to_string_lossy().into_owned()));
                    ctx.request_repaint();
                    last = Instant::now();
                }
            });
            let _ = tx.send(Event::Finished(result));
            ctx.request_repaint();
        });
    }
    pub(super) fn poll_vault_cache(&mut self, ctx: &Context) {
        let entering = self.page == Page::VaultCache && !self.vault.visible;
        self.vault.visible = self.page == Page::VaultCache;
        let mut finished = false;
        loop {
            let event = match self.vault.rx.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(event)) => event,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    if !finished {
                        self.vault.error =
                            Some("Cache scan stopped. Use Refresh cache to try again.".into());
                    }
                    finished = true;
                    break;
                }
                _ => break,
            };
            match event {
                Event::Progress(p) => self.vault.progress = p,
                Event::Finished(result) => {
                    finished = true;
                    if self.vault.root == self.prefs.fab_cache {
                        match result {
                            Ok(inv) => {
                                self.vault.inventory = Some(inv);
                                self.vault.error = None;
                            }
                            Err(e) => self.vault.error = Some(e),
                        }
                    }
                }
            }
        }
        if finished {
            self.vault.rx = None;
        }
        if let Some(result) = self
            .vault
            .remove_rx
            .as_ref()
            .and_then(|rx| match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                    "Cache removal stopped. Refresh to check remaining files.".into(),
                )),
                _ => None,
            })
        {
            self.vault.remove_rx = None;
            self.vault.confirm = None;
            match result {
                Ok(()) => self.notify(
                    "Cached files removed. Installed engines, plugins and projects are unchanged.",
                ),
                Err(e) => self.notify(e),
            }
            self.vault.inventory = None;
            self.scan_vault_cache(ctx);
        }
        if self.vault.root != self.prefs.fab_cache && self.vault.remove_rx.is_none() {
            self.vault.inventory = None;
            self.vault.confirm = None;
            self.vault.error = None;
            if self.vault.rx.is_none() {
                self.vault.root = self.prefs.fab_cache.clone();
            }
        }
        if self.page == Page::VaultCache
            && (self.vault.inventory.is_none() || entering)
            && self.vault.rx.is_none()
            && self.vault.error.is_none()
            && !self.demo
        {
            self.scan_vault_cache(ctx);
        }
    }
    fn vault_busy(&self) -> bool {
        self.transfer.busy()
            || self.plugin_work.busy()
            || self.library_work_pending()
            || self.launcher_updates.blocks_work()
    }
    pub(super) fn vault_cache_page(&mut self, ui: &mut Ui, c: &Canvas) {
        let theme = self.theme.clone();
        let scanning = self.vault.rx.is_some()
            || (self.layout_probe.is_some() && std::env::args().any(|a| a == "--loading-preview"));
        let top = PageHeaderLayout::new().divider_y + 18.;
        let rect = c.rect([271., top, c.width - 297., (c.height - top - 55.).max(160.)]);
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.style_mut().override_font_id = Some(FontId::proportional(16.));
            ui.spacing_mut().item_spacing = vec2(16., 12.);
            ui.spacing_mut().button_padding = vec2(14., 8.);
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let total = self
                    .vault
                    .inventory
                    .as_ref()
                    .map(|i| i.entries.iter().map(|e| e.bytes).sum())
                    .unwrap_or(0);
                let count = self
                    .vault
                    .inventory
                    .as_ref()
                    .map(|i| i.entries.iter().filter(|e| !e.is_metadata()).count())
                    .unwrap_or(0);
                ui.label(
                    RichText::new(
                        if self.vault.inventory.is_none() && self.vault.rx.is_some() {
                            "Calculating cache size…".into()
                        } else {
                            format!("{} in {count} items", size(total))
                        },
                    )
                    .size(20.)
                    .color(color(theme.text)),
                );
                let menu = dots(ui, &theme);
                Popup::menu(&menu).frame(menu_frame(&theme)).show(|ui| {
                    style_menu(ui, &theme);
                    if ui
                        .add_enabled(!scanning, buttons::secondary_button("Refresh cache"))
                        .clicked()
                    {
                        self.scan_vault_cache(ui.ctx());
                        ui.close();
                    }
                    if ui
                        .add(buttons::secondary_button("Show cache folder"))
                        .clicked()
                    {
                        if let Err(e) = platform::open_external(&self.prefs.fab_cache) {
                            self.notify(e);
                        }
                        ui.close();
                    }
                    if ui
                        .add(buttons::secondary_button("Download settings"))
                        .clicked()
                    {
                        self.page = Page::Downloads;
                        self.downloads.settings = true;
                        ui.close();
                    }
                });
            });
            ui.add(
                Label::new(RichText::new(&self.prefs.fab_cache).color(color(theme.muted)))
                    .truncate(),
            )
            .on_hover_text(&self.prefs.fab_cache);
            ui.horizontal_wrapped(|ui| {
                search_field(
                    ui,
                    &mut self.vault.search,
                    "Search cache…",
                    280.,
                    &theme,
                    1.,
                );
                let combo = AppComboBox::new(&theme, "vault-sort")
                    .selected_text(if self.vault.sort_name {
                        "Name A-Z"
                    } else {
                        "Largest first"
                    })
                    .width(160.)
                    .show_ui(ui, |ui| {
                        buttons::selectable_value(
                            ui,
                            &mut self.vault.sort_name,
                            false,
                            "Largest first",
                        );
                        buttons::selectable_value(ui, &mut self.vault.sort_name, true, "Name A-Z");
                    });
                if self.layout_probe.is_some()
                    && std::env::args().any(|a| a == "--control-menu-preview")
                {
                    Popup::open_id(ui.ctx(), combo.response.id.with("popup"));
                }
                if scanning {
                    ui.add(loading::spinner(&theme));
                    ui.label("Scanning cache…")
                        .on_hover_text(&self.vault.progress);
                }
            });
            if let Some(error) = &self.vault.error {
                ui.colored_label(color(theme.warning), error);
            }
            let warnings = self
                .vault
                .inventory
                .as_ref()
                .map(|i| i.warnings.clone())
                .unwrap_or_default();
            if !warnings.is_empty() {
                ui.collapsing(
                    format!(
                        "{} inaccessible paths skipped — sizes may be incomplete",
                        warnings.len()
                    ),
                    |ui| {
                        AppScrollArea::vertical(&theme)
                            .max_height(120.)
                            .id_salt("vault-warnings")
                            .show(ui, |ui| {
                                for warning in &warnings {
                                    ui.label(warning);
                                }
                            });
                    },
                );
            }
            let items = self.library.cache_items();
            let mut entries: Vec<_> = self
                .vault
                .inventory
                .as_ref()
                .into_iter()
                .flat_map(|i| &i.entries)
                .filter(|e| !e.is_metadata())
                .map(|e| {
                    let item = vault_cache::match_item(e, items).cloned();
                    let title = item
                        .as_ref()
                        .map(|i| i.title.clone())
                        .unwrap_or_else(|| e.title.clone());
                    (e.clone(), item, title)
                })
                .collect();
            let search = self.vault.search.to_lowercase();
            entries.retain(|(e, _, title)| {
                title.to_lowercase().contains(&search)
                    || e.path.to_string_lossy().to_lowercase().contains(&search)
            });
            if self.vault.sort_name {
                entries.sort_by_cached_key(|(_, _, title)| title.to_lowercase());
            } else {
                entries.sort_by(|(a, _, _), (b, _, _)| {
                    b.bytes.cmp(&a.bytes).then(a.title.cmp(&b.title))
                });
            }
            AppScrollArea::vertical(&theme)
                .id_salt("vault-cards")
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if entries.is_empty() && self.vault.rx.is_none() && self.vault.error.is_none() {
                        ui.label(if search.is_empty() {
                            "No cached downloads found in this folder."
                        } else {
                            "No cache items match your search."
                        });
                    }
                    let columns = ((ui.available_width() + 16.) / 286.).floor().max(1.) as usize;
                    let width = ((ui.available_width() - 16. * (columns - 1) as f32)
                        / columns as f32)
                        .min(360.);
                    for row in entries.chunks(columns) {
                        ui.horizontal_top(|ui| {
                            for (entry, item, title) in row {
                                self.vault_card(ui, &theme, entry, item.as_ref(), title, width);
                            }
                        });
                    }
                });
        });
        self.vault_remove_dialog(ui.ctx());
        if self.vault.chunk_info {
            let mut open = true;
            dialog::show(
                ui.ctx(),
                &theme,
                "shared-chunks-info",
                "Shared download chunks",
                "Reusable download data saved by this launcher.",
                &mut open,
                |ui| {
                    ui.label("These are verified pieces of downloaded files. The launcher reuses them when installing, updating or repairing engines, plugins and assets, reducing repeat downloads.");
                    ui.label(
                        RichText::new(
                            "Safe to remove when no downloads or installations are running.",
                        )
                        .strong(),
                    );
                    ui.label("Removing these chunks frees disk space. It keeps your installed engines, plugins, projects and separately saved plugin downloads. Future operations may need to download the missing data again and require an internet connection.");
                    ui.label("Cache removal is disabled while this launcher has active or queued transfers.");
                },
            );
            self.vault.chunk_info = open;
        }
    }
    fn vault_card(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        entry: &Entry,
        item: Option<&engine_workspace::fab::Item>,
        title: &str,
        width: f32,
    ) {
        ui.push_id(&entry.path, |ui| {
            ui.allocate_ui_with_layout(vec2(width, 0.), Layout::top_down(Align::Min), |ui| {
                let response = ui.interact(
                    Rect::from_min_size(ui.cursor().min, vec2(width, (width - 2.) * 0.54 + 100.)),
                    ui.id().with("context"),
                    Sense::click(),
                );
                Frame::new()
                    .fill(color(theme.card))
                    .stroke(Stroke::new(1., color(theme.border)))
                    .corner_radius(6.)
                    .show(ui, |ui| {
                        ui.set_width(width - 2.);
                        let (picture, _) = ui.allocate_exact_size(
                            vec2(width - 2., (width - 2.) * 0.54),
                            Sense::hover(),
                        );
                        let key = item.map(|i| format!("fab:{}", i.key()));
                        if let Some(item) = item {
                            if let Some(url) = item.thumbnail() {
                                self.queue_library_thumbnail(key.clone().unwrap(), url, ui.ctx());
                            }
                        }
                        if let Some(tex) = key.as_ref().and_then(|k| self.textures.get(k)) {
                            paint_thumbnail(
                                ui,
                                tex,
                                picture,
                                Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                                thumbnail_top_corners(5.),
                            );
                        } else {
                            ui.painter()
                                .rect_filled(picture, 5., color(theme.selection));
                            downloads::draw_icon(
                                ui,
                                theme,
                                Icon::Database,
                                Rect::from_center_size(
                                    picture.center()
                                        - if entry.title == "Shared download chunks" {
                                            vec2(0., 24.)
                                        } else {
                                            Vec2::ZERO
                                        },
                                    vec2(40., 40.),
                                ),
                                theme.muted,
                            );
                            if entry.title == "Shared download chunks" {
                                ui.painter().text(
                                    picture.center() + vec2(0., 16.),
                                    Align2::CENTER_CENTER,
                                    "Reusable download data",
                                    FontId::proportional(14.),
                                    color(theme.text),
                                );
                                ui.painter().text(
                                    picture.center() + vec2(0., 39.),
                                    Align2::CENTER_CENTER,
                                    "Safe to remove when idle",
                                    FontId::proportional(13.),
                                    color(theme.muted),
                                );
                            }
                        }
                        Frame::new().inner_margin(10).show(ui, |ui| {
                            ui.set_width(width - 22.);
                            ui.set_min_height(64.);
                            ui.add(
                                Label::new(RichText::new(title).color(color(theme.text)))
                                    .truncate(),
                            )
                            .on_hover_text(title);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "{}{}",
                                        size(entry.bytes),
                                        if entry.incomplete { " (partial)" } else { "" }
                                    ))
                                    .color(color(theme.muted)),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let menu = dots(ui, theme);
                                    Popup::menu(&menu)
                                        .frame(menu_frame(theme))
                                        .show(|ui| self.vault_entry_menu(ui, entry, item));
                                });
                            });
                        });
                    });
                Popup::context_menu(&response)
                    .frame(menu_frame(theme))
                    .show(|ui| self.vault_entry_menu(ui, entry, item));
                response.on_hover_text(entry.path.display().to_string());
            });
        });
    }
    fn vault_entry_menu(
        &mut self,
        ui: &mut Ui,
        entry: &Entry,
        item: Option<&engine_workspace::fab::Item>,
    ) {
        style_menu(ui, &self.theme);
        if entry.title == "Shared download chunks"
            && ui
                .add(buttons::secondary_button("About shared download chunks"))
                .clicked()
        {
            self.vault.chunk_info = true;
            ui.close();
        }
        if ui
            .add(buttons::secondary_button("Show in folder"))
            .clicked()
        {
            let folder = if entry.path.is_file() {
                entry.path.parent().unwrap_or(&entry.path)
            } else {
                &entry.path
            };
            if let Err(e) = platform::open_external(&folder.to_string_lossy()) {
                self.notify(e);
            }
            ui.close();
        }
        if let Some(item) = item {
            if ui
                .add(buttons::secondary_button("Open in Library"))
                .clicked()
            {
                self.open_cached_library_item(item);
                ui.close();
            }
        }
        ui.separator();
        if ui.add_enabled(entry.removable && !self.vault_busy() && !self.demo && self.vault.rx.is_none(),buttons::secondary_button("Remove cached files")).on_disabled_hover_text("Wait for transfers and cache scanning to finish. Shared metadata and inaccessible entries cannot be removed here.").clicked() {
            self.vault.confirm=Some(entry.clone()); ui.close();
        }
    }
    fn vault_remove_dialog(&mut self, ctx: &Context) {
        let Some(entry) = self.vault.confirm.clone() else {
            return;
        };
        let mut open = true;
        let mut cancel = false;
        let mut remove = false;
        let theme = self.theme.clone();
        dialog::show(
            ctx,
            &theme,
            "remove-vault-cache",
            "Remove cached files",
            "Free space by removing this cached download.",
            &mut open,
            |ui| {
                ui.label(RichText::new(&entry.title).strong());
                ui.label(entry.path.display().to_string());
                ui.label(format!("Remove {} from the cache? Installed projects, engines and plugins are kept. Future installs may need to download these files again.",size(entry.bytes)));
                if entry.title == "Shared download chunks" {
                    ui.label("Safe to remove while transfers are idle. These reusable pieces of downloaded files help avoid repeat downloads. Removing them keeps installed content and separately saved plugin downloads, but future installs, updates or repairs may need to download the data again.");
                }
                if self.vault.remove_rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.add(loading::spinner(&theme));
                        ui.label("Removing cached files…");
                    });
                } else {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.vault_busy(),
                                buttons::secondary_button("Remove cached files"),
                            )
                            .clicked()
                        {
                            remove = true;
                        }
                        if ui.add(buttons::secondary_button("Cancel")).clicked() {
                            cancel = true;
                        }
                    });
                }
            },
        );
        if remove && self.vault.root == self.prefs.fab_cache {
            let root = PathBuf::from(&self.vault.root);
            let (tx, rx) = mpsc::channel();
            self.vault.remove_rx = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(vault_cache::remove(&root, &entry));
                ctx.request_repaint();
            });
        }
        if (!open || cancel) && self.vault.remove_rx.is_none() {
            self.vault.confirm = None;
        }
    }
}
