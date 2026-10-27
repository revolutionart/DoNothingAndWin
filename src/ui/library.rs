use super::*;
use engine_workspace::fab::{self, Item};
use engine_workspace::fab_listing::{self, DateEvent, Sort};
use engine_workspace::fab_search::{SearchDocument, SearchQuery};
mod plugin_download;
const AVAILABLE_UPDATES_FILTER: usize = 4;
type KindResult = (String, String, Result<fab::project_kind::Detection, String>);

enum Event {
    Cached(Vec<Item>),
    Page(Vec<Item>),
    Status(String),
    AcquisitionError(String),
    Loaded(Result<Vec<Item>, String>),
    Installed(Result<String, String>),
    PluginInstalled(Installation, Result<String, String>),
    PluginBatchFinished(Result<String, String>),
    PluginDownloadProgress(fab::plugin_download::Progress),
    PluginDownloaded(Result<std::path::PathBuf, String>),
}
enum UpdateEvent {
    Plugin(String, Instant, engine_workspace::plugins::Checked),
    Engine(
        String,
        Instant,
        Result<Vec<engine_workspace::plugins::Checked>, String>,
    ),
    Projects(std::collections::HashMap<String, Vec<(String, String)>>),
    Finished,
}
#[derive(Default)]
pub(super) struct Library {
    pub(super) catalogue: engine_workspace::plugins::catalogue::Catalogue,
    pub(super) organization: super::library_folders::FolderView,
    account: String,
    pub(super) install_queue: super::library_queue::Queue,
    items: Vec<Item>,
    rx: Option<Receiver<Event>>,
    kind_rx: Option<Receiver<KindResult>>,
    kind_attempted: std::collections::HashSet<String>,
    kind_errors: std::collections::HashMap<String, String>,
    loading: bool,
    had_saved_items: bool,
    installing: bool,
    install_item: String,
    install_engine: String,
    install_results: usize,
    install_control: Option<engine_workspace::installer::Control>,
    download: plugin_download::Download,
    attempted: bool,
    status: String,
    search: String,
    search_index: std::collections::HashMap<String, SearchDocument>,
    pub filter: usize,
    pub(super) sort: Sort,
    engine_filter: String,
    date_rx: Option<Receiver<DateEvent>>,
    date_status: String,
    acquisition_error: Option<String>,
    date_refresh_pending: bool,
    list_view: bool,
    sync_error: bool,
    pub footer: String,
    updates: std::collections::HashMap<String, Vec<(String, String)>>,
    update_rx: Option<Receiver<UpdateEvent>>,
    create_name: String,
    create_folder: String,
    selected: Option<Item>,
    target: usize,
    engine_targets: std::collections::HashSet<String>,
    project_search: String,
    show_all_projects: bool,
    asset_version: String,
    images: Vec<(String, Receiver<Option<ColorImage>>)>,
    image_attempts: std::collections::HashSet<String>,
}
impl Library {
    pub(super) fn cache_items(&self) -> &[Item] {
        &self.items
    }
    pub(super) fn owns_listing(&self, id: &str) -> bool {
        self.items
            .iter()
            .any(|item| fab_listing::listing_id(item).as_deref() == Some(id))
    }
    pub(super) fn transfer_controls(&self, ui: &mut Ui) {
        if let Some(control) = &self.install_control {
            let stopping = control.cancel.load(std::sync::atomic::Ordering::Relaxed);
            if ui
                .add_enabled(
                    !stopping,
                    buttons::secondary_button(if stopping {
                        "Stopping..."
                    } else {
                        "Stop transfer"
                    }),
                )
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(if self.download.active { "Stop this plugin download and remove its partial output." } else { "Stop the current installation and its remaining engines. Other queued plugins stay in the waiting list." })
                .clicked()
            {
                control.stop();
            }
        }
    }

    pub(super) fn checking_updates(&self) -> bool {
        self.update_rx.is_some()
    }
    fn item_updates(&self, item: &Item) -> Vec<(String, String)> {
        let mut updates = self.catalogue.for_item(item);
        updates.extend(self.updates.get(&item.key()).into_iter().flatten().cloned());
        updates
    }
    fn matches_filter(&self, item: &Item, filter: usize) -> bool {
        if filter == AVAILABLE_UPDATES_FILTER {
            !self.item_updates(item).is_empty()
        } else {
            library_kind(item, filter)
        }
    }
    fn replace_items(&mut self, mut items: Vec<Item>) {
        let previous: std::collections::HashMap<_, _> =
            self.items.iter().map(|i| (i.key(), i)).collect();
        for item in &mut items {
            if let Some(old) = previous.get(&item.key()) {
                if let Some(detection) = &old.project_detection {
                    fab::project_kind::apply(item, detection.clone());
                }
                if item.listing_updated_at.is_none() {
                    item.listing_updated_at = old.listing_updated_at;
                }
                if item.acquired_at.is_none() {
                    item.acquired_at = old.acquired_at;
                }
                if item.listing_search.is_none() {
                    item.listing_search = old.listing_search.clone();
                }
            }
        }
        self.items = items;
    }
}
impl Workspace {
    fn check_library_package_types(&mut self, ctx: &Context) {
        if self.demo
            || self.layout_probe.is_some()
            || self.library.kind_rx.is_some()
            || std::env::args().any(|arg| arg.starts_with("--layout-probe="))
        {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let items: std::collections::VecDeque<_> = self
            .library
            .items
            .iter()
            .filter(|item| fab::project_kind::needs_refresh(item))
            .filter(|item| {
                self.library
                    .kind_attempted
                    .insert(fab::project_kind::signature(item))
            })
            .cloned()
            .collect();
        if items.is_empty() {
            return;
        }
        let work = std::sync::Arc::new(std::sync::Mutex::new(items));
        let (tx, rx) = mpsc::channel();
        self.library.kind_rx = Some(rx);
        // Bound metadata requests independently of transfer workers. No package
        // payload is fetched here, only hash-verified manifest file lists.
        for _ in 0..3 {
            let work = work.clone();
            let session = session.clone();
            let tx = tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                loop {
                    let Some(item) = work.lock().unwrap().pop_front() else {
                        break;
                    };
                    let result = fab::project_kind::detect(&session, &item);
                    if tx
                        .send((item.key(), fab::project_kind::signature(&item), result))
                        .is_err()
                    {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }
    }

    pub(super) fn open_cached_library_item(&mut self, item: &Item) {
        self.library.search = item.title.clone();
        self.library.filter = 0;
        self.library.engine_filter.clear();
        self.library.organization.filter =
            engine_workspace::library_organization::FolderFilter::All;
        self.library.organization.show_hidden = true;
        self.page = Page::Library;
    }
    pub(super) fn has_plugin_updates(&self) -> bool {
        self.engines.iter().any(|e| {
            self.library
                .catalogue
                .rows(&e.path)
                .iter()
                .any(|r| r.has_update())
        })
    }
    pub(super) fn plugin_update_links(&mut self, ui: &mut Ui) {
        let updates: Vec<_> = self
            .engines
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let rows = self.library.catalogue.rows(&e.path);
                let names: Vec<_> = rows
                    .iter()
                    .filter(|r| r.has_update())
                    .map(|r| r.plugin.name.as_str())
                    .collect();
                (!names.is_empty()).then(|| (i, format!("UE {} - {}", e.version, names.join(", "))))
            })
            .collect();
        if updates.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Plugin updates available:").color(color(self.theme.warning)));
            for (i, label) in updates {
                if ui
                    .add(buttons::secondary_button(label))
                    .on_hover_text("View plugin updates for this engine")
                    .clicked()
                {
                    self.show_plugins(i);
                    self.plugin_updates_only = true;
                    self.check_plugins(ui.ctx().clone());
                }
            }
        });
    }

    pub(super) fn queue_library_thumbnail(&mut self, key: String, url: String, ctx: &Context) {
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
            let image = fab::thumbnail_image(&url).ok().map(|image| {
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
    pub(super) fn library_footer_status(&self) -> &str {
        if self.library.loading {
            &self.library.status
        } else if self.library.update_rx.is_some() {
            "Checking installed library content for updates…"
        } else if self.library.date_rx.is_some() {
            &self.library.date_status
        } else if self.library.kind_rx.is_some() {
            "Checking library package types…"
        } else {
            ""
        }
    }

    pub(super) fn library_work_pending(&self) -> bool {
        self.library.installing || !self.library.install_queue.waiting().is_empty()
    }
    pub(super) fn library_transfer_status(&self) -> Option<&str> {
        self.library.installing.then(|| {
            if self
                .library
                .install_control
                .as_ref()
                .is_some_and(|control| control.cancel.load(std::sync::atomic::Ordering::Relaxed))
            {
                "Stopping transfer... Finishing the current request and cleaning up."
            } else {
                self.library.status.as_str()
            }
        })
    }
    pub(super) fn refresh_library(&mut self, ctx: &Context) {
        if self.library.rx.is_some() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        self.library.attempted = true;
        self.library.kind_attempted.clear();
        self.library.kind_errors.clear();
        self.library.loading = true;
        self.library.acquisition_error = None;
        self.library.had_saved_items = !self.library.items.is_empty();
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
            let mut result = fab::library_pages(&session, |items| {
                let _ = tx.send(Event::Page(items.to_vec()));
                crate::window_frame::progress_repaint(&ctx);
            });
            if let Ok(items) = &mut result {
                // Preserve recorded dates if this optional service is temporarily unavailable.
                let cached: std::collections::HashMap<_, _> =
                    fab::load_library_cache(&session.account_id)
                        .into_iter()
                        .map(|item| (item.key(), item.acquired_at))
                        .collect();
                for item in items.iter_mut() {
                    item.acquired_at = cached.get(&item.key()).copied().flatten();
                }
                let _ = tx.send(Event::Status("Loading library acquisition dates…".into()));
                if let Err(error) = fab::acquisition::refresh(&session, items) {
                    let _ = tx.send(Event::AcquisitionError(error));
                }
                let _ = fab::save_library_cache(&session.account_id, items);
            }
            let _ = tx.send(Event::Loaded(result));
            ctx.request_repaint();
        });
    }
    fn rebuild_library_search(&mut self) {
        self.library.search_index = self
            .library
            .items
            .iter()
            .map(|item| (item.key(), SearchDocument::new(item)))
            .collect();
        self.rebuild_cache_health_links();
    }
    fn refresh_library_dates(&mut self, ctx: &Context, retry: bool) {
        if self.library.date_rx.is_some() {
            self.library.date_refresh_pending = true;
            return;
        }
        if self.library.items.is_empty() {
            return;
        }
        let items: Vec<_> = self
            .library
            .items
            .iter()
            .filter(|item| fab_listing::listing_id(item).is_some())
            .cloned()
            .collect();
        let (tx, rx) = mpsc::channel();
        self.library.date_rx = Some(rx);
        self.library.date_status = "Updating search tags and dates in the background…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            fab_listing::refresh_dates(&items, retry, |event| {
                let _ = tx.send(event);
                crate::window_frame::progress_repaint(&ctx);
            })
        });
    }
    pub(super) fn poll_library(&mut self, ctx: &Context) {
        let account = self
            .session
            .as_ref()
            .map(|s| s.account_id.clone())
            .unwrap_or_default();
        if account != self.library.account {
            if let Some(control) = &self.library.install_control {
                control.stop();
            }
            self.library.catalogue.clear();
            let (tx, rx) = mpsc::channel();
            let cached_account = account.clone();
            let context = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Event::Cached(fab::load_library_cache(&cached_account)));
                context.request_repaint();
            });
            self.library = Library {
                rx: Some(rx),
                account,
                sort: self.prefs.library_sort,
                ..Default::default()
            };
            self.textures.retain(|key, _| !key.starts_with("fab:"));
        }
        loop {
            let event = match self.library.rx.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(event)) => event,
                Some(Err(mpsc::TryRecvError::Disconnected)) if self.library.installing => {
                    let result = Err("Transfer worker stopped unexpectedly. Check the destination before retrying.".into());
                    self.record_activity(
                        self.library.install_item.clone(),
                        self.library.install_engine.clone(),
                        &result,
                    );
                    self.library.install_queue.finished();
                    self.library.install_control = None;
                    self.library.installing = false;
                    self.library.download.active = false;
                    self.library.rx = None;
                    self.notify(
                        "Transfer worker stopped unexpectedly. Continuing the waiting list.",
                    );
                    break;
                }
                _ => break,
            };
            match event {
                Event::PluginDownloadProgress(progress) => {
                    self.library.status =
                        format!("{} — {}", self.library.install_item, progress.phase);
                    self.library.download.progress = progress;
                }
                Event::PluginDownloaded(result) => self.finish_plugin_download(result),
                Event::Cached(items) => {
                    self.library.rx = None;
                    self.library.items = items;
                    self.rebuild_library_search();
                    self.refresh_library_dates(ctx, false);
                }
                Event::Page(items) => {
                    self.library.status = format!(
                        "Syncing Fab library in the background… {} items checked",
                        items.len()
                    );
                    // Keep the complete saved library visible until a background sync finishes.
                    if !self.library.had_saved_items {
                        self.library.items = items;
                        self.rebuild_library_search();
                    }
                }
                Event::Status(s) => self.library.status = s,
                Event::AcquisitionError(error) => self.library.acquisition_error = Some(error),
                Event::Loaded(result) => {
                    self.library.loading = false;
                    self.library.rx = None;
                    match result {
                        Ok(items) => {
                            self.library.status =
                                format!("{} items from your Fab account", items.len());
                            self.library.replace_items(items);
                            self.rebuild_library_search();
                            self.library.sync_error = false;
                            self.refresh_library_dates(ctx, false);
                            if self.library.update_rx.is_none() {
                                let session = self.session.clone().unwrap();
                                let engines = self.engines.clone();
                                let items = self.library.items.clone();
                                let (tx, rx) = mpsc::channel();
                                self.library.update_rx = Some(rx);
                                let ctx = ctx.clone();
                                let started = Instant::now();
                                let wait_for_library_builds =
                                    self.prefs.wait_for_library_plugin_builds;
                                std::thread::spawn(move || {
                                    let owned = Ok(items);
                                    for engine in engines {
                                        let result =
                                            engine_workspace::plugins::check_with_library_progress(
                                                &engine,
                                                &session,
                                                &owned,
                                                wait_for_library_builds,
                                                |row| {
                                                    let _ = tx.send(UpdateEvent::Plugin(
                                                        engine.path.clone(),
                                                        started,
                                                        row.clone(),
                                                    ));
                                                    ctx.request_repaint();
                                                },
                                            );
                                        let _ = tx.send(UpdateEvent::Engine(
                                            engine.path,
                                            started,
                                            result,
                                        ));
                                        ctx.request_repaint();
                                    }
                                    let mut updates = std::collections::HashMap::<
                                        String,
                                        Vec<(String, String)>,
                                    >::new();
                                    for (key, message) in
                                        fab::project_updates(&session, owned.as_ref().unwrap())
                                    {
                                        updates
                                            .entry(key)
                                            .or_default()
                                            .push((String::new(), message));
                                    }
                                    let _ = tx.send(UpdateEvent::Projects(updates));
                                    let _ = tx.send(UpdateEvent::Finished);
                                    ctx.request_repaint();
                                });
                            }
                        }
                        Err(e) => {
                            self.library.sync_error = true;
                            self.library.status =
                                format!("Sync failed: {e}. Showing saved library.");
                        }
                    }
                }
                Event::PluginInstalled(engine, result) => {
                    self.library.install_results += 1;
                    self.record_activity(
                        self.library.install_item.clone(),
                        engine.version,
                        &result,
                    );
                    self.plugin_counts.clear();
                    self.workspace_cache.invalidate();
                }
                Event::PluginBatchFinished(result) => {
                    self.library.install_queue.finished();
                    if self.library.install_results == 0 {
                        self.record_activity(
                            self.library.install_item.clone(),
                            self.library.install_engine.clone(),
                            &result,
                        );
                    }
                    self.library.installing = false;
                    self.library.install_control = None;
                    self.library.rx = None;
                    self.library.status = result.unwrap_or_else(|error| {
                        if error.starts_with("Stopped.") {
                            error
                        } else {
                            format!("Installation incomplete: {error}")
                        }
                    });
                    self.notify(self.library.status.clone());
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
                    self.library.install_control = None;
                    self.library.rx = None;
                    self.library.status = result.unwrap_or_else(|e| {
                        if e.starts_with("Stopped.") {
                            e
                        } else {
                            format!("Installation failed: {e}")
                        }
                    });
                    self.plugin_counts.clear();
                    self.workspace_cache.invalidate();
                    self.notify(self.library.status.clone());
                }
            }
        }
        while let Some(event) = self
            .library
            .date_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            match event {
                DateEvent::Batch(dates) => {
                    let dates: std::collections::HashMap<_, _> = dates
                        .into_iter()
                        .map(|(key, date, search)| (key, (date, search)))
                        .collect();
                    for item in &mut self.library.items {
                        if let Some((date, search)) = dates.get(&item.key()) {
                            item.listing_updated_at = *date;
                            item.listing_search = search.clone();
                            self.library
                                .search_index
                                .insert(item.key(), SearchDocument::new(item));
                        }
                    }
                }
                DateEvent::Progress(done, total) => {
                    self.library.date_status =
                        format!("Updating search tags and dates in the background… {done}/{total}");
                }
                DateEvent::Finished(_) => {
                    self.library.date_rx = None;
                    self.library.date_status.clear();
                    if self.library.date_refresh_pending {
                        self.library.date_refresh_pending = false;
                        self.refresh_library_dates(ctx, false);
                    }
                }
            }
        }
        while let Some(event) = self
            .library
            .update_rx
            .as_ref()
            .and_then(|r| r.try_recv().ok())
        {
            match event {
                UpdateEvent::Plugin(path, started, row) => {
                    self.library.catalogue.checked_row(path, started, row)
                }
                UpdateEvent::Engine(path, started, Ok(rows)) => {
                    self.library.catalogue.checked(path, started, rows)
                }
                UpdateEvent::Engine(_, _, Err(_)) => {}
                UpdateEvent::Projects(updates) => {
                    for entries in self.library.updates.values_mut() {
                        entries.retain(|(path, _)| !path.is_empty());
                    }
                    for (key, entries) in updates {
                        self.library.updates.entry(key).or_default().extend(entries);
                    }
                }
                UpdateEvent::Finished => self.library.update_rx = None,
            }
        }
        loop {
            let event = match self.library.kind_rx.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(event)) => event,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.library.kind_rx = None;
                    break;
                }
                _ => break,
            };
            let (key, signature, result) = event;
            let Some(item) =
                self.library.items.iter_mut().find(|item| {
                    item.key() == key && fab::project_kind::signature(item) == signature
                })
            else {
                continue;
            };
            match result {
                Ok(detection) => {
                    fab::project_kind::apply(item, detection.clone());
                    if let Some(selected) =
                        self.library.selected.as_mut().filter(|s| s.key() == key)
                    {
                        fab::project_kind::apply(selected, detection);
                    }
                    self.library.kind_errors.remove(&key);
                }
                Err(error) => {
                    self.library.kind_errors.insert(key, error);
                }
            }
        }
        self.check_library_package_types(ctx);
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
        self.start_next_library_install(ctx);
        // Sync once after startup/sign-in, regardless of the visible page. A successful
        // sync checks installed plugins for every engine and updates their card badges.
        // Let sign-in/token refresh and installations finish before starting this work.
        if !self.demo
            && self.session.is_some()
            && self.pending.is_none()
            && !self.library.attempted
            && !self.library_work_pending()
            && !self.transfer.busy()
            && !self.plugin_work.busy()
        {
            self.refresh_library(ctx);
        }
        if !crate::window_frame::is_backgrounded()
            && (self.library.rx.is_some()
                || self.library.kind_rx.is_some()
                || self.library.date_rx.is_some()
                || !self.library.images.is_empty())
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    pub(super) fn real_library_page(&mut self, ui: &mut Ui, c: &Canvas) {
        let header = PageHeaderLayout::new();
        let header_y = header.actions_y;
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
        let top = header.filters_y();
        if self.session.is_none() {
            ui.put(
                c.rect([271., top + 20., 500., 40.]),
                Label::new("Connect your Epic account in Settings to load your Fab library."),
            );
            return;
        }
        let theme = self.theme.clone();
        let mut choose = None;
        let mut update_item = None;
        let rect = c.rect([271., top, c.width - 297., c.height - top - 72.]);
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.style_mut().override_font_id = Some(FontId::proportional(16.));
            ui.spacing_mut().button_padding = vec2(14., 8.);
            ui.spacing_mut().item_spacing = vec2(16., FILTER_ROW_GAP);
            ui.spacing_mut().interact_size.y = FILTER_ROW_HEIGHT;
            let mut visible_items: Vec<_> = self.library.items.iter().enumerate()
                .filter_map(|(index, item)| fab_listing::visible_in_library(item).then_some(index)).collect();
            let keys: Vec<_> = visible_items.iter().map(|&index| self.library.items[index].key()).collect();
            self.library_folder_toolbar(ui, &keys);
            let organization = self.library_organization();
            visible_items.retain(|&index| organization.matches(&self.library.items[index].key(), &self.library.organization.filter, self.library.organization.show_hidden));
            let branches = fab_listing::engine_branches(visible_items.iter().map(|&index| &self.library.items[index]));
            ui.horizontal_wrapped(|ui| {
                ui.style_mut().override_font_id = Some(FontId::proportional(14. * c.scale()));
                ui.spacing_mut().button_padding = vec2(10., 5.) * c.scale();
                ui.spacing_mut().item_spacing = vec2(8., 8.) * c.scale();
                ui.spacing_mut().interact_size.y = 32. * c.scale();
                for (index, name) in [(0, "All"), (1, "Assets"), (3, "Projects"), (2, "Plugins"), (AVAILABLE_UPDATES_FILTER, "Available Updates")] {
                    let count = visible_items
                        .iter()
                        .map(|&index| &self.library.items[index])
                        .filter(|i| self.library.matches_filter(i, index))
                        .count();
                    let selected = self.library.filter == index;
                    let response = buttons::tab_button(ui, &theme, selected, format!("{name} ({count})"), 0.);
                    if response
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.library.filter = index;
                    }
                }
                let fixed_controls_width = (140. + 140. + 32. + 32. + 4. * 8.) * c.scale();
                let search_width = (rect.width() - fixed_controls_width).clamp(140. * c.scale(), 258. * c.scale());
                let controls_width = search_width + fixed_controls_width;
                if rect.right() - ui.cursor().left() < controls_width {
                    ui.end_row();
                }
                ui.add_space((rect.right() - ui.cursor().left() - controls_width).max(0.));
                search_field(
                    ui,
                    &mut self.library.search,
                    "Search library...",
                    search_width,
                    &theme,
                    c.scale(),
                ).on_hover_text("Search titles, descriptions, tags, and publishers. Title matches appear first; your chosen sort applies within each group.");
                AppComboBox::new(&self.theme, "library-engine")
                    .width(140. * c.scale())
                    .selected_text(if self.library.engine_filter.is_empty() {"All engines".into()} else {format!("UE {}",self.library.engine_filter)})
                    .show_ui(ui, |ui| {
                        buttons::selectable_value(ui, &mut self.library.engine_filter,String::new(),"All engines").on_hover_cursor(CursorIcon::PointingHand);
                        for branch in &branches {
                            buttons::selectable_value(ui, &mut self.library.engine_filter,branch.clone(),format!("UE {branch}")).on_hover_cursor(CursorIcon::PointingHand);
                        }
                    }).response.on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text("Show content that lists support for this engine version on Windows.");
                AppComboBox::new(&self.theme, "library-sort")
                    .width(140. * c.scale())
                    .selected_text(self.library.sort.label())
                    .show_ui(ui, |ui| {
                        for sort in [Sort::LatestUpdate,Sort::LatestAdded,Sort::NameAscending,Sort::NameDescending] {
                            buttons::selectable_value(ui, &mut self.library.sort,sort,sort.label()).on_hover_cursor(CursorIcon::PointingHand);
                        }
                    }).response.on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text("Latest added uses your Fab library acquisition dates, newest first. Latest update uses listing update dates. Unavailable dates appear last.");
                for (list, icon) in [(false, Icon::Grid), (true, Icon::List)] {
                    let (r, response) =
                        ui.allocate_exact_size(Vec2::splat(COMPACT_BUTTON_HEIGHT) * c.scale(), Sense::click());
                    ui.painter().rect_filled(
                        r,
                        BUTTON_RADIUS,
                        color(if self.library.list_view == list {
                            if response.hovered() || response.is_pointer_button_down_on() {
                                theme.accent_hover
                            } else { theme.accent }
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
                        (32. - VIEW_MODE_ICON_SIZE) / 2.,
                        (32. - VIEW_MODE_ICON_SIZE) / 2.,
                        VIEW_MODE_ICON_SIZE,
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
            self.plugin_update_links(ui);
            if self.library.sort == Sort::LatestAdded {
                let dated = visible_items.iter().filter(|&&i| self.library.items[i].acquired_at.is_some()).count();
                ui.label(RichText::new(format!("Acquisition dates available for {dated} of {} items. Items without a date appear last.", visible_items.len())).color(color(theme.muted)));
                if let Some(error) = &self.library.acquisition_error {
                    ui.colored_label(color(theme.warning), format!("Could not refresh acquisition dates: {error}. Use Sync library to retry."));
                }
            }
            if self.library.sync_error { ui.colored_label(color(theme.warning), &self.library.status); }
            let query = SearchQuery::new(&self.library.search);
            let mut items: Vec<_> = visible_items
                .into_iter()
                .map(|index| &self.library.items[index])
                .filter(|i| {
                    self.library.search_index.get(&i.key()).and_then(|doc| query.rank(doc)).is_some()
                        && self.library.matches_filter(i, self.library.filter)
                        && fab_listing::supports_engine(i, &self.library.engine_filter)
                })
                .cloned()
                .collect();
            fab_listing::sort(&mut items, self.library.sort);
            // Stable relevance grouping preserves the selected ordering within each group.
            if !query.is_empty() {
                items.sort_by_cached_key(|i| self.library.search_index.get(&i.key()).and_then(|doc| query.rank(doc)));
            }
            self.library.footer = format!("{} items", items.len());
            let list = self.library.list_view;
            let (columns, card_width) = card_grid_metrics(ui.available_width() - scrollbar::GRID_ALLOCATED_WIDTH, LIBRARY_CARD_WIDTH, PROJECT_CARD_GAP);
            let thumbnail_height = (card_width - 2.) / LIBRARY_THUMBNAIL_ASPECT;
            if !list { ui.spacing_mut().item_spacing = vec2(PROJECT_CARD_GAP, PROJECT_CARD_GAP); }
            AppScrollArea::vertical(&self.theme)
                .inner_margin(scrollbar::GRID_MARGIN)
                .id_salt((
                    "fab-owned-items",
                    self.library.organization.filter.clone(),
                    self.library.organization.show_hidden,
                    self.library.filter,
                    self.library.search.clone(),
                    self.library.sort,
                    self.library.engine_filter.clone(),
                    self.library.list_view,
                ))
                .max_height(ui.available_height())
                .auto_shrink([false, false])
                .show_rows(
                    ui,
                    if list { 124. } else { thumbnail_height + 84. },
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
                                        card_width
                                    };
                                    let height = if list { 120. } else { thumbnail_height + 84. };
                                    let (r, _) =
                                        ui.allocate_exact_size(vec2(width, height), Sense::hover());
                                    let mut card_ui = ui.new_child(UiBuilder::new().max_rect(r));
                                    let ui = &mut card_ui;
                                    let item_updates = self.library.item_updates(item);
                                    let updates = (!item_updates.is_empty()).then_some(&item_updates);
                                    let has_update = updates.is_some_and(|u| !u.is_empty());
                                    let updating = self.library.catalogue.rows_for_item(item)
                                        .into_iter()
                                        .any(|(path, row)| self.plugin_work.is_updating(path, &row.plugin));
                                    let update_color = color(theme.warning);
                                    ui.painter().rect_filled(r, 6., color(theme.card));
                                    ui.painter().rect_stroke(
                                        r,
                                        6.,
                                        Stroke::new(1., if has_update { update_color } else { color(theme.border) }),
                                        StrokeKind::Inside,
                                    );
                                    let picture = if list {
                                        Rect::from_min_size(r.min + vec2(8., 8.), vec2(116., 104.))
                                    } else {
                                        Rect::from_min_size(
                                            r.min + vec2(1., 1.),
                                            vec2(width - 2., thumbnail_height),
                                        )
                                    };
                                    let thumbnail = ui.interact(picture, ui.id().with((&key, "thumbnail")), Sense::click());
                                    let show_controls = ui.rect_contains_pointer(r) || thumbnail.has_focus()
                                        || Popup::is_id_open(ui.ctx(), ui.id().with((&key, "folder-menu")));
                                    let picture_corners = if list { CornerRadius::same(5) } else { thumbnail_top_corners(5.) };
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
                                        paint_thumbnail(ui, tex, picture, uv, picture_corners);
                                    } else {
                                        ui.painter().rect_filled(
                                            picture,
                                            picture_corners,
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
                                            r.min + vec2(10., thumbnail_height + 9.),
                                            vec2(width - 24., 25.),
                                        )
                                    };
                                    let title_response = ui.interact(
                                        title,
                                        ui.id().with((&key, "title")),
                                        Sense::click(),
                                    ).on_hover_cursor(CursorIcon::PointingHand);
                                    let title_color = color(if title_response.hovered() { theme.accent_hover } else { theme.text });
                                    let mut job = egui::text::LayoutJob::simple_singleline(
                                        item.title.clone(),
                                        FontId::proportional(if list { 16. } else { 14. }),
                                        title_color,
                                    );
                                    job.wrap.max_width = title.width();
                                    job.wrap.max_rows = 1;
                                    job.wrap.break_anywhere = true;
                                    let galley = ui.painter().layout_job(job);
                                    ui.painter().galley(
                                        pos2(title.left(), title.center().y - galley.size().y / 2.),
                                        galley,
                                        title_color,
                                    );
                                    let listing = fab_listing::listing_id(item);
                                    let action = if listing.is_some() { "Open listing on Fab" } else { "Search for this title on Fab" };
                                    if title_response.on_hover_text(format!("{}\n{action}\nLast updated: {} (Fab listing)\nAcquired: {}", item.title, fab_listing::date_label(item.listing_updated_at), fab_listing::date_label(item.acquired_at))).clicked() {
                                        let url = if let Some(id) = listing {
                                            format!("https://www.fab.com/listings/{id}")
                                        } else {
                                            let mut url = reqwest::Url::parse("https://www.fab.com/search").expect("static Fab URL");
                                            url.query_pairs_mut().append_pair("q", &item.title);
                                            url.into()
                                        };
                                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                                    }
                                    let y = if list { r.center().y } else { r.bottom() - 28. };
                                    let badge = Rect::from_min_size(
                                        pos2(
                                            if list {
                                                r.left() + 140.
                                            } else {
                                                r.left() + 10.
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
                                    } else if item.complete_project() {
                                        "Project"
                                    } else {
                                        "Asset"
                                    };
                                    library_type_icon(
                                        ui,
                                        badge.min + vec2(10., 7.),
                                        kind,
                                        color(self.cache_icon_tint(item)),
                                    );
                                    ui.painter().text(
                                        badge.min + vec2(33., 15.),
                                        Align2::LEFT_CENTER,
                                        kind,
                                        FontId::proportional(14.),
                                        color(theme.text),
                                    );
                                    let badge_response = ui.interact(badge, ui.id().with((&key,"cache-health")), Sense::click())
                                        .on_hover_ui(|ui| { ui.label(self.cached_item_tooltip(item)); });
                                    let badge_menu_id = badge_response.id.with("context-menu");
                                    let menu_preview = std::env::args().any(|a| a == "--cache-health-library-menu-preview") && item.asset_id == "item-0";
                                    if menu_preview {
                                        Popup::open_id(ui.ctx(), badge_menu_id);
                                    }
                                    let mut badge_menu = Popup::context_menu(&badge_response).id(badge_menu_id).frame(menu_frame(&theme));
                                    if menu_preview {
                                        badge_menu = badge_menu.anchor(badge).align(RectAlign::TOP_START);
                                    }
                                    badge_menu.show(|ui| self.library_item_options(ui, &key, &organization));
                                    if show_controls {
                                    let info = Rect::from_center_size(
                                        pos2(picture.right() - 20., picture.top() + 20.),
                                        vec2(28., 28.),
                                    );
                                    let tint = if has_update {
                                        update_color
                                    } else {
                                        color(theme.text)
                                    };
                                    ui.painter().circle_filled(info.center(), 14., color(theme.card));
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
                                    }
                                    let unknown_kind = !fab::project_kind::known(item);
                                    let type_error = self.library.kind_errors.get(&item.key()).filter(|_| unknown_kind);
                                    let checking_kind = unknown_kind && type_error.is_none() && !self.demo;
                                    let label = library_action_label(item, updating, has_update, checking_kind, type_error.is_some());
                                    let text = ui.painter().layout_no_wrap(label.into(),
                                        buttons::primary_font(1.), color(theme.on_accent));
                                    let spinner_space = if updating { loading::SIZE + 6. } else { 0. };
                                    let action_width = (if list { 146_f32 } else { 126_f32 })
                                        .max(text.size().x + spinner_space + 2. * buttons::HORIZONTAL_PADDING);
                                    let action = Rect::from_min_size(
                                        pos2(r.right() - if list { 12. } else { 10. } - action_width,
                                            y - COMPACT_BUTTON_HEIGHT / 2.),
                                        vec2(action_width, COMPACT_BUTTON_HEIGHT),
                                    );
                                    let response = ui
                                        .interact(action, ui.id().with((&key, "install")),
                                            if updating || checking_kind { Sense::hover() } else { Sense::click() })
                                        .on_hover_cursor(if updating || checking_kind { CursorIcon::Default } else { CursorIcon::PointingHand });
                                    let response = if let Some(error) = type_error {
                                        response.on_hover_text(format!("Could not check package type: {error}\nClick to retry. No project files have been changed."))
                                    } else { response };
                                    response.widget_info(|| WidgetInfo::labeled(
                                        WidgetType::Button, response.enabled() && !updating && !checking_kind, label));
                                    let update_action = has_update && item.plugin();
                                    let update_colors = buttons::update_colors(&theme, response.hovered() && !updating);
                                    let fill = if update_action {
                                        color(update_colors.0)
                                    } else {
                                        buttons::primary_fill(&theme, &response)
                                    };
                                    ui.painter().rect_filled(action, BUTTON_RADIUS, fill);
                                    let text_color = if update_action { color(update_colors.1) } else { color(theme.on_accent) };
                                    let content_left = action.center().x - (text.size().x + spinner_space) / 2.;
                                    if updating {
                                        ui.put(
                                            Rect::from_center_size(pos2(content_left + loading::SIZE / 2., action.center().y), Vec2::splat(loading::SIZE)),
                                            loading::spinner(&theme),
                                        );
                                    }
                                    ui.painter().galley_with_override_text_color(
                                        pos2(content_left + spinner_space, action.center().y - text.size().y / 2.), text,
                                        text_color,
                                    );
                                    if response.clicked()
                                        && !updating
                                        && !checking_kind
                                        && (!self.library.installing || (item.plugin() && !has_update))
                                    {
                                        if unknown_kind && !self.demo {
                                            self.library.kind_errors.remove(&item.key());
                                            self.library.kind_attempted.remove(&fab::project_kind::signature(item));
                                        } else if has_update && item.plugin() {
                                            update_item = Some(item.clone());
                                        } else {
                                            choose = Some(item.clone());
                                        }
                                    }
                                    self.library_item_menu(ui, picture, &thumbnail, item, &organization, show_controls);
                                }
                            });
                            if list { ui.add_space(4.); }
                        }
                        if items.is_empty() && !self.library.loading {
                            ui.label(if self.library.filter == AVAILABLE_UPDATES_FILTER {
                                "No available updates match the current filters."
                            } else if self.library.organization.show_hidden {
                                "No hidden items match the current filters."
                            } else {
                                "No matching Library items. Try another folder or search, or enable Show hidden."
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
        if let Some(item) = update_item {
            let pending = self
                .library
                .catalogue
                .rows_for_item(&item)
                .into_iter()
                .find_map(|(path, row)| {
                    let update = row.update.as_ref()?;
                    let engine = self.engines.iter().find(|e| e.path == path)?;
                    Some((engine.clone(), row.plugin.clone(), update.clone()))
                });
            if let Some((engine, plugin, update)) = pending {
                if self.start_plugin_updates(engine, vec![(plugin, update)], ui.ctx().clone()) {
                    self.page = Page::Downloads;
                    self.downloads.show_overview();
                } else {
                    self.notify("Finish the current operation before updating this plugin.");
                }
            } else {
                self.notify(
                    "No direct update is available. Check the plugin's update notice for details.",
                );
            }
        }
        if let Some(item) = choose {
            self.reset_plugin_download_cache();
            self.library.create_name = item
                .title
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(48)
                .collect();
            self.library.engine_targets.clear();
            self.library.download.only = false;
            self.library.download.branch = item
                .supported_branches()
                .last()
                .cloned()
                .unwrap_or_default();
            if item.plugin() {
                if let Some(engine) = self.engines.iter().find(|engine| {
                    item.compatible(&engine.version).is_some()
                        && (self.demo || platform::engine_ready(engine))
                }) {
                    self.library.engine_targets.insert(engine.path.clone());
                }
            }
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
            .is_some_and(Item::complete_project)
        {
            self.library_create_dialog(ui.ctx());
        } else {
            self.library_install_dialog(ui.ctx());
        }
        self.library_folder_dialog(ui.ctx());
    }
    fn start_next_library_install(&mut self, ctx: &Context) {
        let ready = !self.demo
            && self.session.is_some()
            && self.library.rx.is_none()
            && !self.library.installing
            && self.pending.is_none()
            && !self.transfer.busy()
            && !self.plugin_work.busy()
            && !self.launcher_updates.blocks_work();
        let Some(job) = self.library.install_queue.start_next(ready) else {
            return;
        };
        let session = self
            .session
            .clone()
            .expect("queue dispatch requires a session");
        let item = job.item;
        let engines = job.engines;
        let (tx, rx) = mpsc::channel();
        self.library.rx = Some(rx);
        self.library.installing = true;
        let control = engine_workspace::installer::Control::default();
        self.library.install_control = Some(control.clone());
        self.library.install_results = 0;
        self.library.install_item = format!("Installation: {}", item.title);
        self.library.install_engine = engines
            .iter()
            .map(|e| e.version.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        self.library.status = "Preparing plugin installations...".into();
        let wait_for_library = self.prefs.wait_for_library_plugin_builds;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = fab::plugin_install::install_many(
                &session,
                &item,
                &engines,
                &control,
                wait_for_library,
                |status| {
                    let _ = tx.send(Event::Status(status));
                    crate::window_frame::progress_repaint(&ctx);
                },
                |engine, result| {
                    let _ = tx.send(Event::PluginInstalled(engine.clone(), result.clone()));
                    ctx.request_repaint();
                },
            );
            let _ = tx.send(Event::PluginBatchFinished(result));
            ctx.request_repaint();
        });
    }
    pub(super) fn library_install_dialog(&mut self, ctx: &Context) {
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
        if item.plugin() {
            self.library
                .engine_targets
                .retain(|path| targets.iter().any(|t| &t.1 == path));
        }
        let waiting = self.pending.is_some()
            || self.library.rx.is_some()
            || self.library.installing
            || self.transfer.busy()
            || self.plugin_work.busy()
            || !self.library.install_queue.waiting().is_empty();
        let mut open = true;
        let mut install = false;
        let mut download = false;
        dialog::show(
            ctx,
            &self.theme.clone(),
            "fab-install",
            if item.plugin() {
                if self.library.download.only {
                    "Download plugin"
                } else {
                    "Install to engine"
                }
            } else {
                "Add to project"
            },
            if item.plugin() {
                "Install this plugin or download its files for use elsewhere."
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
                                vec2(64., 64.)
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
                    ui.horizontal(|ui| {
                        buttons::selectable_value(
                            ui,
                            &mut self.library.download.only,
                            false,
                            "Install to engine",
                        );
                        buttons::selectable_value(
                            ui,
                            &mut self.library.download.only,
                            true,
                            "Download only",
                        );
                    });
                    ui.add_space(8.);
                    if self.library.download.only {
                        download = self.plugin_download_options(ui, &item, waiting);
                        return;
                    }
                    ui.label("Compatible engines");
                    if targets.is_empty() {
                        ui.add_enabled(
                            false,
                            buttons::secondary_button("No compatible engines installed")
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
                        AppScrollArea::vertical(&self.theme)
                            .id_salt("fab-engine-picker")
                            .max_height(180.)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for (name, path, _) in &targets {
                                    ui.push_id(path, |ui| {
                                        let mut selected =
                                            self.library.engine_targets.contains(path);
                                        engine_install_row(
                                            ui,
                                            &self.theme,
                                            name,
                                            path,
                                            &mut selected,
                                        );
                                        if selected {
                                            self.library.engine_targets.insert(path.clone());
                                        } else {
                                            self.library.engine_targets.remove(path);
                                        }
                                    });
                                }
                            });
                        ui.label("Each selected engine receives its latest compatible plugin build. Versions may differ between engines.");
                        ui.label(
                            RichText::new(if waiting {
                                "Queued plugins start automatically after earlier jobs finish."
                            } else {
                                "Already installed plugins can be updated using Update."
                            })
                            .color(color(self.theme.muted)),
                        );
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
                        ui.add(buttons::checkbox(
                            &mut self.library.show_all_projects,
                            "Show all projects",
                            &self.theme,
                        ))
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
                    AppScrollArea::vertical(&self.theme).id_salt("fab-project-picker").max_height((ctx.content_rect().height()-530.).clamp(120.,300.)).auto_shrink([false,true]).show(ui,|ui|{
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
                            if let Some(tex)=artwork{paint_thumbnail(ui,tex,picture,Rect::from_min_max(Pos2::ZERO,pos2(1.,1.)),thumbnail_top_corners(3.));}
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
                                    AppComboBox::new(&self.theme, "fab-alternative-version")
                                        .selected_text(if self.library.asset_version.is_empty() {
                                            "Select version"
                                        } else {
                                            &self.library.asset_version
                                        })
                                        .show_ui(ui, |ui| {
                                            for branch in branches {
                                                buttons::selectable_value(
                                                    ui,
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
                let new_targets = targets
                    .iter()
                    .filter(|target| {
                        self.library.engine_targets.contains(&target.1)
                            && !self.library.install_queue.contains(&item, &target.1)
                    })
                    .count();
                let valid = if item.plugin() {
                    new_targets > 0
                } else {
                    targets.get(self.library.target).is_some_and(|t| {
                        item.import_version(&t.2, &self.library.asset_version)
                            .is_some()
                    })
                };
                if item.plugin() && new_targets < self.library.engine_targets.len() {
                    ui.label("Selections already installing or queued will not be added twice.");
                }
                let install_label = if item.plugin() {
                    format!(
                        "{} ({new_targets})",
                        if waiting { "Add to queue" } else { "Install" }
                    )
                } else {
                    "Add to project".into()
                };
                install = dialog::footer(
                    ui,
                    &self.theme,
                    &install_label,
                    valid
                        && !self.demo
                        && self.session.is_some()
                        && (item.plugin()
                            || (self.library.rx.is_none()
                                && !self.transfer.busy()
                                && !self.plugin_work.busy()))
                        && !self.launcher_updates.blocks_work(),
                    false,
                );
            },
        );
        if !open {
            self.library.selected = None;
        }
        if download {
            self.start_plugin_download(item, ctx);
            return;
        }
        if install && item.plugin() {
            let engines = self
                .engines
                .iter()
                .filter(|engine| self.library.engine_targets.contains(&engine.path))
                .cloned()
                .collect();
            let title = item.title.clone();
            let added = self.library.install_queue.enqueue(item, engines);
            self.library.selected = None;
            if added > 0 {
                self.notify(format!(
                    "Added {title} to the install queue for {added} engine(s)."
                ));
            }
            self.start_next_library_install(ctx);
        } else if install {
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
            let control = engine_workspace::installer::Control::default();
            self.library.install_control = Some(control.clone());
            self.library.install_item = format!("Installation: {}", item.title);
            self.library.install_engine = engine.clone();
            self.library.selected = None;
            self.library.status = "Preparing download...".into();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = fab::install_with_control(
                    &session,
                    &item,
                    &version,
                    Path::new(&target),
                    &engine,
                    &control,
                    |s| {
                        let _ = tx.send(Event::Status(s));
                        crate::window_frame::progress_repaint(&ctx);
                    },
                );
                let _ = tx.send(Event::Installed(result));
                ctx.request_repaint();
            });
        }
    }
}

fn engine_install_row(
    ui: &mut Ui,
    theme: &Theme,
    name: &str,
    path: &str,
    selected: &mut bool,
) -> Response {
    Frame::new()
        .fill(color(theme.card))
        .stroke(Stroke::new(
            1.,
            color(if *selected {
                theme.accent
            } else {
                theme.border
            }),
        ))
        .corner_radius(5.)
        .inner_margin(6)
        .show(ui, |ui| {
            buttons::compact_style(ui);
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let response = ui
                    .add(buttons::checkbox(selected, name, theme))
                    .on_hover_text(path);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("Engine installed")
                            .size(14.)
                            .color(color(theme.muted)),
                    );
                });
                response
            })
            .inner
        })
        .inner
}

fn library_kind(item: &Item, filter: usize) -> bool {
    match filter {
        1 => item.distribution_method == "ASSET_PACK" && !item.complete_project(),
        2 => item.plugin(),
        3 => item.complete_project(),
        _ => true,
    }
}
fn library_action_label(
    item: &Item,
    updating: bool,
    has_update: bool,
    checking: bool,
    failed: bool,
) -> &'static str {
    if checking {
        "Checking package…"
    } else if failed {
        "Check package"
    } else if updating {
        "Updating"
    } else if has_update && item.plugin() {
        "Update"
    } else if item.plugin() {
        "Install to engine"
    } else if item.complete_project() {
        "Create project"
    } else {
        "Add to project"
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
                        .add(buttons::secondary_button("Browse"))
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.library.create_folder = p.display().to_string();
                        }
                    }
                });
                AppComboBox::new(&self.theme, "template-version")
                    .selected_text(if self.library.asset_version.is_empty() {
                        "Choose engine version"
                    } else {
                        &self.library.asset_version
                    })
                    .show_ui(ui, |ui| {
                        for branch in item.supported_branches() {
                            buttons::selectable_value(
                                ui,
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
            let control = engine_workspace::installer::Control::default();
            self.library.install_control = Some(control.clone());
            self.library.install_item = format!("Project creation: {}", item.title);
            self.library.install_engine = branch.clone();
            let (tx, rx) = mpsc::channel();
            self.library.rx = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = fab::create_project_with_control(
                    &session,
                    &item,
                    &version,
                    Path::new(&folder),
                    &name,
                    &branch,
                    &control,
                    |s| {
                        let _ = tx.send(Event::Status(s));
                        crate::window_frame::progress_repaint(&ctx);
                    },
                );
                let _ = tx.send(Event::Installed(result));
                ctx.request_repaint();
            });
        }
    }
}

#[cfg(test)]
mod update_state_tests {
    use super::*;

    #[test]
    fn manifest_evidence_controls_card_action_and_category_for_any_asset_pack() {
        let mut city: Item = serde_json::from_value(serde_json::json!({
            "assetId":"any-id",
            "assetNamespace":"any-publisher",
            "title":"Mislabelled full project",
            "distributionMethod":"ASSET_PACK",
            "url":"https://www.fab.com/listings/4898e707-7855-404b-af0e-a505ee690e68"
        }))
        .unwrap();
        city.project_detection = Some(
            serde_json::from_value(serde_json::json!({
                "fingerprint": fab::project_kind::signature(&city),
                "contains_project": true, "checked_at": engine_workspace::epic::now(),
            }))
            .unwrap(),
        );
        assert_eq!(
            library_action_label(&city, false, false, false, false),
            "Create project"
        );
        assert!(
            library_kind(&city, 3),
            "Any verified project package must use the Projects category"
        );
        assert!(
            !library_kind(&city, 1),
            "Project packages must not appear as importable assets"
        );
        city.title = "Renamed listing".into();
        assert!(
            library_kind(&city, 3),
            "Manifest evidence must survive a title change"
        );
        city.asset_id = "separate-buildings-pack".into();
        city.title = "City Sample Buildings".into();
        assert!(!library_kind(&city, 3));
        assert!(
            library_kind(&city, 1),
            "Separate asset packs must remain importable"
        );
        assert_eq!(
            library_action_label(&city, false, false, true, false),
            "Checking package…"
        );
        assert_eq!(
            library_action_label(&city, false, false, false, true),
            "Check package"
        );
    }
    fn item() -> Item {
        serde_json::from_value(serde_json::json!({"assetId":"example", "assetNamespace":"publisher", "title":"Example", "distributionMethod":"CODE_PLUGIN"})).unwrap()
    }
    #[test]
    fn library_transfer_has_stop_button_and_keeps_busy_until_worker_finishes() {
        let ctx = Context::default();
        let control = engine_workspace::installer::Control::default();
        let library = Library {
            installing: true,
            install_control: Some(control.clone()),
            ..Default::default()
        };
        let mut button = Rect::NOTHING;
        for frame in 0..3 {
            let mut input = RawInput::default();
            if frame > 0 {
                let pos = button.center();
                input.events.push(egui::Event::PointerMoved(pos));
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: frame == 1,
                    modifiers: Modifiers::default(),
                });
            }
            let output = ctx.run(input, |ctx| {
                CentralPanel::default().show(ctx, |ui| library.transfer_controls(ui));
            });
            let text = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Text(t) if t.galley.job.text == "Stop transfer" => Some(t),
                    _ => None,
                })
                .expect("active library installation must show Stop transfer");
            button = Rect::from_min_size(text.pos, text.galley.size());
        }
        assert!(control.cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(
            library.installing,
            "busy protection must stay active during cleanup"
        );
        let output = ctx.run(RawInput::default(), |ctx| {
            CentralPanel::default().show(ctx, |ui| library.transfer_controls(ui));
        });
        assert!(
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape, Shape::Text(t) if t.galley.job.text == "Stopping..."))
        );
    }
    #[test]
    fn engine_picker_keeps_multiple_selections_and_readable_hover_status() {
        let ctx = Context::default();
        let theme = Theme::dark();
        let mut selected = [false, false];
        let mut rects = [Rect::NOTHING; 2];
        // Select both engines, then unselect only the first. Hover and click each real checkbox.
        for frame in 0..7 {
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(680., 400.))),
                ..Default::default()
            };
            if frame > 0 {
                let index = if frame <= 2 || frame >= 5 { 0 } else { 1 };
                let pos = rects[index].center();
                input.events.push(egui::Event::PointerMoved(pos));
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: frame % 2 == 1,
                    modifiers: Modifiers::default(),
                });
            }
            let output = ctx.run(input, |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    *ui.visuals_mut() = theme.visuals();
                    // Reproduce the default open-widget black foreground that caused the bug.
                    ui.visuals_mut().widgets.open.fg_stroke.color = Color32::BLACK;
                    for index in 0..2 {
                        ui.push_id(index, |ui| {
                            rects[index] = engine_install_row(
                                ui,
                                &theme,
                                if index == 0 {
                                    "Unreal Engine 5.6.1"
                                } else {
                                    "Unreal Engine 5.8.3"
                                },
                                "test engine",
                                &mut selected[index],
                            )
                            .rect;
                        });
                    }
                });
            });
            let labels: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    Shape::Text(text) if text.galley.job.text == "Engine installed" => Some(text),
                    _ => None,
                })
                .collect();
            assert_eq!(labels.len(), 2);
            for label in labels {
                assert!(
                    label
                        .galley
                        .job
                        .sections
                        .iter()
                        .all(|section| section.format.color == color(theme.muted))
                );
            }
            if frame == 4 {
                assert_eq!(selected, [true, true]);
            }
        }
        assert_eq!(selected, [false, true]);
    }
    #[test]
    fn available_updates_filter_matches_actual_updates_across_content_types() {
        let mut library = Library::default();
        for kind in ["CODE_PLUGIN", "ASSET_PACK", "COMPLETE_PROJECT"] {
            let mut content = item();
            content.distribution_method = kind.into();
            content.asset_id = kind.into();
            assert!(!library.matches_filter(&content, AVAILABLE_UPDATES_FILTER));
            library.updates.insert(content.key(), vec![]);
            assert!(!library.matches_filter(&content, AVAILABLE_UPDATES_FILTER));
            library.updates.insert(
                content.key(),
                vec![
                    ("engine-a".into(), "Update available".into()),
                    ("engine-b".into(), "Update available".into()),
                ],
            );
            assert!(library.matches_filter(&content, AVAILABLE_UPDATES_FILTER));
            assert!(library.matches_filter(&content, 0));
            assert_eq!(library.matches_filter(&content, 2), kind == "CODE_PLUGIN");
            library.updates.get_mut(&content.key()).unwrap().clear();
            assert!(!library.matches_filter(&content, AVAILABLE_UPDATES_FILTER));
        }
    }
    #[test]
    fn sync_keeps_loaded_search_details_and_removes_unowned_items() {
        let mut old = item();
        old.listing_updated_at = Some(123);
        old.acquired_at = Some(42);
        old.listing_search = Some(fab::SearchMetadata {
            description: "monster".into(),
            ..Default::default()
        });
        let mut removed = old.clone();
        removed.asset_id = "removed".into();
        let mut library = Library {
            items: vec![old, removed],
            ..Default::default()
        };
        library.replace_items(vec![item()]);
        assert_eq!(library.items.len(), 1);
        assert_eq!(library.items[0].listing_updated_at, Some(123));
        assert_eq!(library.items[0].acquired_at, Some(42));
        assert_eq!(
            library.items[0]
                .listing_search
                .as_ref()
                .unwrap()
                .description,
            "monster"
        );
    }
}

impl Workspace {
    // Explicit render-only fixture; no credentials, requests or installed content.
    pub(super) fn preview_library_organization(&mut self, step: usize) {
        let account = "library-organization-preview".to_owned();
        self.session = Some(engine_workspace::epic::Session {
            account_id: account.clone(),
            display_name: "Preview".into(),
            refresh_token: String::new(),
        });
        self.library = Library {
            account: account.clone(),
            attempted: true,
            list_view: step == 2,
            sort: self.prefs.library_sort,
            ..Default::default()
        };
        for (index, (title, kind)) in [
            ("Forest environment collection", "ASSET_PACK"),
            ("Editor workflow tools", "CODE_PLUGIN"),
            ("Adventure game starter", "COMPLETE_PROJECT"),
            ("Hidden material library", "ASSET_PACK"),
        ]
        .into_iter()
        .enumerate()
        {
            self.library.items.push(serde_json::from_value(serde_json::json!({
                "assetId":format!("item-{index}"), "assetNamespace":"preview", "title":title,
                "distributionMethod":kind,
                "url":format!("https://www.fab.com/listings/00000000-0000-0000-0000-{index:012}"),
                "projectVersions":[{"artifactId":"preview", "engineVersions":["5.8"], "targetPlatforms":["Windows"]}]
            })).unwrap());
        }
        for item in &mut self.library.items {
            if item.distribution_method == "ASSET_PACK" {
                item.project_detection = Some(serde_json::from_value(serde_json::json!({
                    "fingerprint": fab::project_kind::signature(item), "contains_project": false,
                    "checked_at": engine_workspace::epic::now(),
                })).unwrap());
            }
        }
        let mut org = engine_workspace::library_organization::Organization::default();
        org.create_folder("Environments").unwrap();
        org.create_folder("Tools and plugins").unwrap();
        org.move_item("preview:item-0", Some("Environments"))
            .unwrap();
        org.move_item("preview:item-1", Some("Tools and plugins"))
            .unwrap();
        org.set_hidden("preview:item-3", true);
        self.prefs.library_organization.insert(account, org);
        self.library.organization.preview(step);
        if std::env::args().any(|a| a == "--cache-health-library-menu-preview") {
            self.library.filter = 1;
            self.library.organization.show_hidden = false;
        }
        if std::env::args().any(|a| a == "--library-acquisition-preview") {
            self.library.sort = Sort::LatestAdded;
            self.library.items[0].acquired_at = Some(1704067200);
            self.library.items[1].acquired_at = Some(1735689600);
        }
        // Optional local fixture for rendering metadata regressions. This only
        // runs in the explicit render-only library preview with a fake account.
        if let Some(path) = std::env::args().find_map(|arg| {
            arg.strip_prefix("--library-item-preview=")
                .map(str::to_owned)
        }) {
            let item: Item =
                serde_json::from_slice(&std::fs::read(path).expect("Read library preview item"))
                    .expect("Parse library preview item");
            if std::env::args().any(|arg| arg == "--library-package-check-failed-preview") {
                self.library
                    .kind_errors
                    .insert(item.key(), "Temporary network failure".into());
            }
            if std::env::args().any(|arg| arg == "--library-item-dialog-preview") {
                self.library.selected = Some(item.clone());
                self.library.create_name = "SampleProject".into();
                self.library.create_folder = "C:/Preview/Projects".into();
            }
            self.library.items = vec![item];
            self.library.organization.filter =
                engine_workspace::library_organization::FolderFilter::All;
            self.library.organization.show_hidden = false;
        }
        self.rebuild_library_search();
    }
}

impl Workspace {
    // Render-only multi-engine fixture: no sign-in, network calls or installation.
    pub(super) fn preview_plugin_install(&mut self) {
        self.demo = true;
        self.preview_library_organization(0);
        self.engines = ["5.8.3", "5.7.4", "5.6.1"]
            .into_iter()
            .map(|version| Installation {
                version: version.into(),
                path: format!("C:/Preview/UE_{version}"),
                association: version.into(),
            })
            .collect();
        let item: Item = serde_json::from_value(serde_json::json!({
            "assetId":"preview-plugin", "assetNamespace":"preview", "title":"Multi-engine plugin installation",
            "distributionMethod":"CODE_PLUGIN", "projectVersions":[
                {"artifactId":"preview58", "engineVersions":["5.8"], "targetPlatforms":["Windows"]},
                {"artifactId":"preview57", "engineVersions":["5.7"], "targetPlatforms":["Windows"]},
                {"artifactId":"preview56", "engineVersions":["5.6"], "targetPlatforms":["Windows"]}
            ]
        })).unwrap();
        self.library.engine_targets = self
            .engines
            .iter()
            .filter(|e| e.version != "5.7.4")
            .map(|e| e.path.clone())
            .collect();
        self.library.selected = Some(item);
    }
}

impl Workspace {
    pub(super) fn preview_library_queue(&mut self) {
        self.preview_plugin_install();
        let mut first = self.library.selected.clone().unwrap();
        first.asset_id = "active-preview".into();
        first.title = "Installing example plugin".into();
        self.library
            .install_queue
            .enqueue(first, vec![self.engines[0].clone()]);
        self.library.install_queue.start_next(true);
        for (id, title) in [
            ("waiting-tools", "Editor workflow tools"),
            (
                "waiting-framework",
                "Ascent Combat Framework Ultimate - C++ Complete Action RPG Creator",
            ),
        ] {
            let mut item = self.library.selected.clone().unwrap();
            item.asset_id = id.into();
            item.title = title.into();
            self.library
                .install_queue
                .enqueue(item, vec![self.engines[0].clone(), self.engines[2].clone()]);
        }
        self.library.installing = true;
        self.library.install_control = Some(engine_workspace::installer::Control::default());
        self.library.status = "Example plugin - UE 5.8.3 - Downloading verified files...".into();
    }
    pub(super) fn finish_library_preview(&mut self) {
        self.library.installing = false;
        self.library.install_control = None;
        self.library.install_queue = Default::default();
    }
    pub(super) fn preview_install_progress(&mut self, streaming: bool) {
        self.library.installing = true;
        self.library.install_control = Some(engine_workspace::installer::Control::default());
        let file = if streaming { 1471 } else { 1472 };
        self.library.status = format!(
            "PCGEx • Version {} • UE 5.8.4 • Engine 1 of 1 • Preparing file {file} / 7076 - 20.8%\n{}",
            engine_workspace::plugins::display_version("0.79.3+0.1"),
            fab::transfer_totals(
                2_824_509_988,
                13_579_374_942,
                515_000_000,
                Some(2_829_795_131)
            )
        );
    }
}
