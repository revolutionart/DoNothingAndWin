mod buttons;
mod categories;
mod clone_project;
mod dialog;
mod downloads;
mod engine_options;
mod engine_order;
mod free_offers;
mod launcher_updates;
mod layout_probe;
mod library;
mod library_folders;
mod library_queue;
mod loading;
mod plugin_activity;
mod plugin_jobs;
mod plugins_view;
mod project_backup;
mod project_cards;
mod project_finder;
mod project_thumbnails;
mod project_visibility;
mod scrollbar;
mod settings_flow;
mod transfers;
mod vault_cache;
mod workspace_cache;
use buttons::primary_button;
use eframe::egui::emath::GuiRounding;
use eframe::egui::{self, *};
use engine_workspace::{
    epic::{self, Epic, Session},
    model::*,
    platform,
    storage::{self, Preferences},
    theme::{Theme, color},
};
use scrollbar::{AppComboBox, AppScrollArea};
use std::{
    collections::HashMap,
    path::Path,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Workspace,
    Projects,
    Library,
    FreeOffers,
    VaultCache,
    Engines,
    Downloads,
    Settings,
}
#[derive(Clone, Copy, Default, PartialEq)]
enum ProjectSort {
    #[default]
    RecentlyOpened,
    NameAscending,
    NameDescending,
}
impl ProjectSort {
    fn label(self) -> &'static str {
        match self {
            Self::RecentlyOpened => "Recently opened",
            Self::NameAscending => "Name A-Z",
            Self::NameDescending => "Name Z-A",
        }
    }
}
#[derive(Clone, Copy)]
enum Icon {
    User,
    Cube,
    Home,
    Folder,
    Chip,
    Download,
    Book,
    Gear,
    Plus,
    Upload,
    Search,
    Grid,
    List,
    Down,
    Dots,
    Pin,
    Check,
    Layers,
    Storage,
    Speed,
    Database,
    Back,
    Reset,
}
enum Job {
    Checked(Result<(Snapshot, Option<Session>), String>),
    SignedIn(Result<Option<Session>, String>),
}
struct Toast {
    message: String,
    created: Instant,
    scan_results: bool,
}
pub struct Workspace {
    page: Page,
    theme: Theme,
    prefs: Preferences,
    engines: Vec<Installation>,
    projects: Vec<Project>,
    demo: bool,
    session: Option<Session>,
    snapshot: Snapshot,
    status: String,
    error: Option<String>,
    textures: HashMap<String, TextureHandle>,
    project_thumbnails: project_thumbnails::Thumbnails,
    search: String,
    pinned_only: bool,
    category_filter: Option<String>,
    category_manager: bool,
    category_add_project: Option<String>,
    category_name: String,
    filter: String,
    project_sort: ProjectSort,
    list_view: bool,
    pending: Option<Receiver<Job>>,
    next_check: Instant,
    failures: u32,
    login: bool,
    launch_path: Option<String>,
    engine_options: Option<Installation>,
    options: engine_options::Options,
    engine_remove: Option<Installation>,
    cloning: clone_project::Cloning,
    backup: project_backup::Backup,
    new_project: bool,
    new_name: String,
    new_path: String,
    new_engine: usize,
    selected: Option<usize>,
    toast: Option<Toast>,
    free: Option<u64>,
    project_scroll: f32,
    layout_probe: Option<layout_probe::LayoutProbe>,
    transfer: transfers::Transfers,
    scan: transfers::Scan,
    project_finder: project_finder::Finder,
    engine_space: settings_flow::EngineSpace,
    plugin_engine: Option<usize>,
    plugin_work: plugin_jobs::PluginWork,
    library: library::Library,
    free_offers: free_offers::FreeOffers,
    vault: vault_cache::Vault,
    downloads: downloads::Downloads,
    plugin_search: String,
    plugin_updates_only: bool,
    plugins: Vec<engine_workspace::plugins::Plugin>,
    plugin_counts: HashMap<String, usize>,
    workspace_cache: workspace_cache::WorkspaceCache,
    launcher_updates: launcher_updates::LauncherUpdates,
}
const BUTTON_SCALE: f32 = 0.75;
const COMPACT_BUTTON_HEIGHT: f32 = 32.;
const COMPACT_BUTTON_FONT_SIZE: f32 = 14.;
const BUTTON_RADIUS: f32 = 5. * BUTTON_SCALE;
const VIEW_MODE_ICON_SIZE: f32 = 16.;
fn table_row_hover_color(theme: &Theme) -> Color32 {
    color(if theme.dark {
        [29, 34, 38]
    } else {
        [232, 236, 241]
    })
}
fn button_bounds([x, y, w, h]: [f32; 4]) -> [f32; 4] {
    [
        x + w * (1. - BUTTON_SCALE) / 2.,
        y + h * (1. - BUTTON_SCALE) / 2.,
        w * BUTTON_SCALE,
        h * BUTTON_SCALE,
    ]
}
// Map a desired visible button rectangle to the shared compact-button slot.
fn button_slot([x, y, w, h]: [f32; 4]) -> [f32; 4] {
    let sw = w / BUTTON_SCALE;
    let sh = h / BUTTON_SCALE;
    [x - (sw - w) / 2., y - (sh - h) / 2., sw, sh]
}
// Shared compact search control, matching the Projects toolbar.
fn search_field(
    ui: &mut Ui,
    value: &mut String,
    hint: &str,
    width: f32,
    theme: &Theme,
    scale: f32,
) -> Response {
    let height = 32. * scale;
    let (rect, slot) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.painter()
        .rect_filled(rect, BUTTON_RADIUS, color(theme.card));
    let has_text = !value.is_empty();
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
    let font = FontId::proportional(COMPACT_BUTTON_FONT_SIZE * scale);
    // Use one stable reference for input and hint so typing descenders does not
    // move the baseline. Caret geometry follows the same shifted row.
    let reference = ui
        .painter()
        .layout_no_wrap("Hg".into(), font.clone(), color(theme.text));
    let offset = reference.rect.center().y - reference.mesh_bounds.center().y;
    let align_input = |mut galley: std::sync::Arc<egui::Galley>| {
        let adjusted = std::sync::Arc::make_mut(&mut galley);
        for row in &mut adjusted.rows {
            row.pos.y += offset;
        }
        adjusted.mesh_bounds = adjusted.mesh_bounds.translate(vec2(0., offset));
        galley
    };
    let hint = align_input(ui.painter().layout_no_wrap(
        hint.into(),
        font.clone(),
        color(theme.muted),
    ));
    let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, _width: f32| {
        align_input(ui.painter().layout_no_wrap(
            text.as_str().into(),
            font.clone(),
            color(theme.text),
        ))
    };
    let input_rect = Rect::from_min_max(
        rect.min + vec2(34., 3.) * scale,
        rect.max - vec2(if has_text { 30. } else { 8. }, 3.) * scale,
    );
    let mut response = child.put(
        input_rect,
        TextEdit::singleline(value)
            .id(slot.id.with("input"))
            .hint_text(hint)
            .font(font.clone())
            .layouter(&mut layouter)
            .text_color(color(theme.text))
            .vertical_align(Align::Center)
            .margin(Vec2::ZERO)
            .frame(false),
    );
    // Paint the frame behind the text; the interior stays transparent.
    ui.painter().rect_stroke(
        rect,
        BUTTON_RADIUS,
        Stroke::new(
            1.,
            color(if response.has_focus() {
                theme.accent
            } else {
                theme.border
            }),
        ),
        StrokeKind::Inside,
    );
    let canvas = Canvas {
        origin: rect.min,
        width: width / scale,
        height: 32.,
        sx: scale,
        sy: scale,
        theme: theme.clone(),
    };
    canvas.icon(ui, Icon::Search, 12., 8., 16., theme.muted);
    if has_text {
        let clear = ui
            .interact(
                Rect::from_min_max(
                    pos2(rect.right() - 29. * scale, rect.top() + 3. * scale),
                    rect.max - vec2(3., 3.) * scale,
                ),
                slot.id.with("clear"),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Clear search");
        clear.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Clear search")
        });
        ui.painter().text(
            clear.rect.center() - vec2(0., scale),
            Align2::CENTER_CENTER,
            "×",
            FontId::proportional(20. * scale),
            color(if clear.hovered() {
                theme.text
            } else {
                theme.muted
            }),
        );
        if clear.clicked() {
            value.clear();
            response.mark_changed();
            response.request_focus();
        }
    }
    response
}
fn highlighted_panel(theme: &Theme) -> Frame {
    Frame::new()
        .fill(color(theme.card))
        .stroke(Stroke::new(1.5, color(theme.accent)))
        .corner_radius(6.)
        .inner_margin(22)
}

fn menu_frame(theme: &Theme) -> Frame {
    Frame::popup(&Style {
        visuals: theme.visuals(),
        ..Default::default()
    })
    .inner_margin(Margin::same(8))
}
fn style_menu(ui: &mut Ui, theme: &Theme) {
    style_menu_settings(ui.style_mut(), theme);
}
fn style_menu_settings(style: &mut Style, theme: &Theme) {
    style.override_font_id = Some(FontId::proportional(COMPACT_BUTTON_FONT_SIZE));
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.button_padding = vec2(10., 5.);
    style.spacing.item_spacing = vec2(8., 4.);
    style.spacing.interact_size.y = 32.;
    style.visuals.button_frame = true;
    style.visuals.override_text_color = None;
    style.visuals.selection.bg_fill = color(theme.accent);
    style.visuals.selection.stroke.color = color(theme.on_accent);
    let widgets = &mut style.visuals.widgets;
    widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    widgets.inactive.bg_stroke = Stroke::NONE;
    widgets.inactive.fg_stroke.color = color(theme.text);
    widgets.inactive.corner_radius = CornerRadius::same(5);
    for w in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
        w.weak_bg_fill = color(theme.accent);
        w.bg_fill = color(theme.accent);
        w.fg_stroke.color = color(theme.on_accent);
        w.bg_stroke = Stroke::NONE;
        w.corner_radius = CornerRadius::same(5);
        w.expansion = 0.;
    }
}

impl Workspace {
    fn launch_button(&mut self, ui: &mut Ui, c: &Canvas, r: [f32; 4]) {
        let chosen = self
            .engines
            .iter()
            .position(|e| e.path == self.prefs.default_engine)
            .or_else(|| latest_installation(&self.engines));
        let mut r = button_bounds(r);
        r[1] += (r[3] - COMPACT_BUTTON_HEIGHT) / 2.;
        r[3] = COMPACT_BUTTON_HEIGHT;
        let [x, y, w, h] = r;
        let split = 42. * BUTTON_SCALE;
        let launch = ui
            .interact(
                c.rect([x, y, w - split, h]),
                Id::new("header-launch"),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        let menu = ui
            .interact(
                c.rect([x + w - split, y, split, h]),
                Id::new("header-launch-menu"),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        let t = &self.theme;
        c.fill(
            ui,
            r,
            if launch.hovered() || menu.hovered() {
                t.launch_hover
            } else {
                t.launch_background
            },
            BUTTON_RADIUS,
        );
        c.line(
            ui,
            [x + w - split, y],
            [x + w - split, y + h],
            t.launch_foreground,
        );
        let label = chosen
            .map(|i| {
                format!(
                    "Launch UE {}",
                    EngineVersion::parse(&self.engines[i].version)
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| self.engines[i].version.clone())
                )
            })
            .unwrap_or_else(|| "Install engine".into());
        c.control_text(
            ui,
            x + (w - split) / 2.,
            y + h / 2.,
            &label,
            true,
            t.launch_foreground,
            Align2::CENTER_CENTER,
        );
        ui.painter().add(Shape::convex_polygon(
            vec![
                c.point(x + w - 31. * BUTTON_SCALE, y + h / 2. - 5. * BUTTON_SCALE),
                c.point(x + w - 11. * BUTTON_SCALE, y + h / 2. - 5. * BUTTON_SCALE),
                c.point(x + w - 21. * BUTTON_SCALE, y + h / 2. + 6. * BUTTON_SCALE),
            ],
            color(t.launch_foreground),
            Stroke::NONE,
        ));
        if launch
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            if let Some(i) = chosen {
                self.launch_engine(i);
            } else {
                self.page = Page::Engines;
            }
        }
        if let Some(i) = chosen {
            launch.on_hover_text(format!("Open Unreal Editor\n{}", self.engines[i].path));
        }
        let previous_launch = self.launch_path.clone();
        egui::Popup::menu(&menu)
            .frame(menu_frame(&self.theme))
            .show(|ui| {
                style_menu(ui, &self.theme);
                ui.set_min_width(260.);
                if ui
                    .selectable_label(
                        self.launch_path.is_none(),
                        "Always use latest installed engine",
                    )
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.launch_path = None;
                    self.prefs.default_engine.clear();
                    self.persist();
                    ui.close();
                }
                ui.separator();
                for e in &self.engines {
                    if ui
                        .selectable_label(
                            self.launch_path.as_deref() == Some(&e.path),
                            format!("Unreal Engine {}", e.version),
                        )
                        .on_hover_text(&e.path)
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.launch_path = Some(e.path.clone());
                        self.prefs.default_engine = e.path.clone();
                        ui.close();
                    }
                }
                ui.separator();
                if ui
                    .add(buttons::secondary_button("Install engine…"))
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.page = Page::Engines;
                    ui.close();
                }
            });
        if previous_launch != self.launch_path {
            self.persist();
        }
    }
    pub fn new(cc: &eframe::CreationContext<'_>, demo: bool, light: bool) -> Self {
        let mut fonts = FontDefinitions::default();
        for (name, path) in [
            ("segoe", "C:/Windows/Fonts/segoeui.ttf"),
            ("semibold", "C:/Windows/Fonts/seguisb.ttf"),
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert(name.into(), FontData::from_owned(bytes).into());
                if name == "segoe" {
                    fonts
                        .families
                        .get_mut(&FontFamily::Proportional)
                        .unwrap()
                        .insert(0, name.into());
                } else {
                    fonts
                        .families
                        .insert(FontFamily::Name(name.into()), vec![name.into()]);
                }
            }
        }
        if !fonts
            .families
            .contains_key(&FontFamily::Name("semibold".into()))
        {
            fonts.families.insert(
                FontFamily::Name("semibold".into()),
                fonts.families[&FontFamily::Proportional].clone(),
            );
        }
        cc.egui_ctx.set_fonts(fonts);
        cc.egui_ctx.options_mut(|options| {
            // Faster wheel travel across pages and dialogs; retain egui's smoothing
            // and the native pixel deltas used by precision touchpads.
            options.input_options.line_scroll_speed = 100.;
        });
        cc.egui_ctx.style_mut(|style| {
            style.spacing.button_padding = vec2(buttons::HORIZONTAL_PADDING, 5.);
            style.spacing.interact_size.y = COMPACT_BUTTON_HEIGHT;
            style.text_styles.insert(
                TextStyle::Button,
                FontId::proportional(COMPACT_BUTTON_FONT_SIZE),
            );
        });
        let mut prefs = storage::load_preferences();
        let startup = engine_workspace::download_policy::startup_settings();
        prefs.run_at_startup = startup.is_some();
        if let Some(in_tray) = startup {
            prefs.startup_in_tray = in_tray;
        }
        if light {
            prefs.theme = "light".into();
        }
        let theme = if prefs.theme == "light" {
            Theme::light()
        } else {
            Theme::dark()
        };
        theme.apply(&cc.egui_ctx);
        let engines = platform::display_engines();
        let session = storage::load_session();
        let account = session
            .as_ref()
            .map(|s| s.account_id.as_str())
            .unwrap_or("public");
        let snapshot = storage::load_snapshot(account);
        let roots = engine_workspace::discovery::project_roots(&prefs.project_roots);
        engine_workspace::discovery::retain_project_roots(
            &mut prefs.projects,
            &roots,
            &prefs.manual_projects,
        );
        let _ = storage::save_preferences(&prefs);
        let mut app = Self {
            page: Page::Workspace,
            theme,
            prefs: prefs.clone(),
            engines,
            projects: prefs.projects.clone(),
            demo,
            session,
            snapshot,
            status: "Checking engine releases…".into(),
            error: None,
            textures: HashMap::new(),
            project_thumbnails: Default::default(),
            search: String::new(),
            pinned_only: false,
            category_filter: None,
            category_manager: false,
            category_add_project: None,
            category_name: String::new(),
            filter: "All versions".into(),
            project_sort: ProjectSort::default(),
            list_view: false,
            pending: None,
            next_check: Instant::now()
                + if std::env::args().any(|a| a.starts_with("--layout-probe=")) {
                    Duration::from_secs(86400)
                } else {
                    Duration::ZERO
                },
            failures: 0,
            login: std::env::args().any(|a| a == "--sign-in"),
            launch_path: prefs.launch_engine.clone(),
            engine_options: None,
            options: Default::default(),
            engine_remove: None,
            cloning: Default::default(),
            backup: Default::default(),
            free_offers: Default::default(),
            vault: Default::default(),
            new_project: false,
            new_name: String::new(),
            new_path: String::new(),
            new_engine: 0,
            selected: None,
            toast: None,
            free: platform::free_space(),
            project_scroll: 0.0,
            layout_probe: layout_probe::LayoutProbe::from_args(),
            transfer: transfers::Transfers::default(),
            scan: transfers::Scan::default(),
            project_finder: Default::default(),
            engine_space: settings_flow::EngineSpace::default(),
            plugin_engine: None,
            plugin_work: Default::default(),
            library: Default::default(),
            downloads: downloads::Downloads::load(),
            plugin_search: String::new(),
            plugin_updates_only: false,
            plugins: Vec::new(),
            plugin_counts: HashMap::new(),
            workspace_cache: Default::default(),
            launcher_updates: Default::default(),
        };
        app.load_assets(&cc.egui_ctx);
        if !demo && app.layout_probe.is_none() {
            app.check(cc.egui_ctx.clone());
        } else if demo {
            app.status = "Design preview • sample engines and projects".into();
        } else {
            app.status = "Layout verification • cached engine data".into();
        }
        if !demo
            && app.prefs.scan_projects_on_startup
            && app.layout_probe.is_none()
            && !std::env::args().any(|a| a == "--no-scan")
        {
            app.start_scan(cc.egui_ctx.clone(), None);
        }
        if std::env::args().any(|a| a == "--show-plugins") && !app.engines.is_empty() {
            app.show_plugins((app.engines.len() - 1).min(1));
        }
        app
    }
    fn load_assets(&mut self, ctx: &Context) {
        for (name, bytes, mask) in [
            (
                "brand-light",
                include_bytes!("../assets/brand_light.png").as_slice(),
                false,
            ),
            (
                "brand",
                include_bytes!("../assets/brand.png").as_slice(),
                false,
            ),
        ] {
            let mut im = image::load_from_memory(bytes).unwrap().to_rgba8();
            if mask {
                for p in im.pixels_mut() {
                    let a = p[0].min(p[1]).min(p[2]);
                    let a = ((a as f32 - 65.0) / 190.0 * 255.0).clamp(0.0, 255.0) as u8;
                    *p = image::Rgba([255, 255, 255, a]);
                }
            }
            let size = [im.width() as usize, im.height() as usize];
            self.textures.insert(
                name.into(),
                ctx.load_texture(
                    name,
                    ColorImage::from_rgba_unmultiplied(size, im.as_raw()),
                    TextureOptions::LINEAR,
                ),
            );
        }
        if !self.demo {
            self.project_thumbnails.invalidate();
            self.poll_project_thumbnails(ctx);
            return;
        }
        for p in &mut self.projects {
            if let Some(path) = &p.artwork {
                if self.textures.contains_key(path) {
                    continue;
                }
                if let Ok(im) = image::open(path) {
                    let im = im.thumbnail(820, 360).to_rgba8();
                    self.textures.insert(
                        path.clone(),
                        ctx.load_texture(
                            path,
                            ColorImage::from_rgba_unmultiplied(
                                [im.width() as usize, im.height() as usize],
                                im.as_raw(),
                            ),
                            TextureOptions::LINEAR,
                        ),
                    );
                }
            }
        }
    }
    fn notify(&mut self, s: impl Into<String>) {
        self.toast = Some(Toast {
            message: s.into(),
            created: Instant::now(),
            scan_results: false,
        });
    }
    fn persist(&mut self) {
        if self.demo {
            return;
        }
        self.prefs.launch_engine = self.launch_path.clone();
        self.prefs.projects = self.projects.clone();
        if let Err(e) = storage::save_preferences(&self.prefs) {
            self.error = Some(e);
        }
    }
    fn check(&mut self, ctx: Context) {
        if self.pending.is_some() || self.transfer.preparing() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        // Reload in case the last refresh rotated the token before a later request failed.
        let session = if self.session.is_some() {
            storage::load_session()
        } else {
            None
        };
        self.pending = Some(rx);
        self.status = "Checking engine releases…".into();
        std::thread::spawn(move || {
            let result = Epic::new().and_then(|e| e.check(session.as_ref()));
            let _ = tx.send(Job::Checked(result));
            ctx.request_repaint();
        });
    }
    fn sign_in(&mut self, ctx: Context) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.login = false;
        self.error = None;
        self.status = "Finish signing in in the Epic window…".into();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let result = engine_workspace::login::connect();
            let _ = tx.send(Job::SignedIn(result));
            ctx.request_repaint();
        });
    }
    fn poll(&mut self, ctx: &Context) {
        let job = self.pending.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(job) = job {
            self.pending = None;
            match job {
                Job::SignedIn(Ok(Some(session))) => {
                    self.snapshot = storage::load_snapshot(&session.account_id);
                    self.prefs.display_name = session.display_name.clone();
                    self.session = Some(session);
                    self.login = false;
                    self.error = None;
                    self.persist();
                    self.check(ctx.clone());
                }
                Job::SignedIn(Ok(None)) => {
                    self.status = "Sign-in cancelled".into();
                }
                Job::SignedIn(Err(e)) => {
                    self.error = Some(e);
                }
                Job::Checked(Ok((mut snapshot, session))) => {
                    let old_seen = self.snapshot.seen.clone();
                    snapshot.merge_history(&self.snapshot);
                    let changed = snapshot
                        .notices
                        .iter()
                        .rev()
                        .find(|n| {
                            !old_seen.contains(&format!("Windows:Live:{}:{}", n.app, n.current))
                        })
                        .cloned();
                    self.snapshot = snapshot;
                    if let Some(change) = changed {
                        self.notify(format!(
                            "{}: {:?} available - {}",
                            change.app, change.kind, change.current
                        ));
                        ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(
                            egui::UserAttentionType::Informational,
                        ));
                    }
                    if let Some(s) = session {
                        self.prefs.display_name = s.display_name.clone();
                        self.session = Some(s);
                    }
                    self.failures = 0;
                    self.error = None;
                    self.status = if self.snapshot.verified() {
                        format!(
                            "{} engine builds checked • hotfix tracking active",
                            self.snapshot.releases.len()
                        )
                    } else if self.session.is_none() {
                        "Engine catalog checked • sign in to check hotfixes".into()
                    } else {
                        format!(
                            "Check incomplete • {} builds unavailable",
                            self.snapshot.missing_builds.len()
                        )
                    };
                    let account = self
                        .session
                        .as_ref()
                        .map(|s| s.account_id.as_str())
                        .unwrap_or("public");
                    if let Err(e) = storage::save_snapshot(account, &self.snapshot) {
                        self.error = Some(e)
                    }
                    self.next_check = Instant::now()
                        + Duration::from_secs(
                            self.prefs.check_minutes.clamp(5, 1440) * 60 + epic::now() % 31,
                        );
                }
                Job::Checked(Err(e)) => {
                    self.failures = (self.failures + 1).min(5);
                    self.status = "Release check failed • previous results retained".into();
                    self.error = Some(e);
                    self.next_check =
                        Instant::now() + Duration::from_secs(900 * 2u64.pow(self.failures - 1));
                }
            }
        }
        if !self.demo
            && self.pending.is_none()
            && !self.transfer.preparing()
            && Instant::now() >= self.next_check
        {
            self.check(ctx.clone());
        }
    }
    fn import(&mut self, ctx: &Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Unreal project", &["uproject"])
            .pick_file()
        {
            match platform::import_project(&path) {
                Ok(p) => {
                    if self.projects.iter().any(|x| x.path == p.path) {
                        self.notify("This project is already in your workspace.");
                    } else {
                        if self.demo {
                            self.demo = false;
                            self.projects.clear();
                            self.engines = platform::display_engines();
                        }
                        self.prefs.manual_projects.push(p.path.clone());
                        self.projects.push(p);
                        self.load_assets(ctx);
                        self.persist();
                    }
                }
                Err(e) => self.notify(e),
            }
        }
    }
    fn open_project(&mut self, index: usize) {
        if self.transfer.operation.kind() != engine_workspace::engine_operation::Kind::Install
            && self.transfer.busy()
        {
            self.notify("Wait for engine maintenance to finish before opening a project.");
            return;
        }
        if self.demo {
            self.notify("Design preview. Import a real .uproject to launch it.");
            return;
        }
        let p = &self.projects[index];
        let engine = project_engine(p, &self.engines).map(|i| &self.engines[i]);
        match engine {
            Some(e) => match platform::launch(e, Some(p)) {
                Ok(()) => {
                    self.projects[index].last_opened = epic::now();
                    self.persist();
                    self.notify("Opening project in Unreal Editor…");
                }
                Err(e) => self.notify(e),
            },
            None => self.notify(
                "The project's engine is not installed. Add its engine folder in Settings.",
            ),
        }
    }
    fn launch_engine(&mut self, index: usize) {
        if self.transfer.operation.kind() != engine_workspace::engine_operation::Kind::Install
            && self.transfer.busy()
        {
            self.notify("Wait for engine maintenance to finish before launching.");
            return;
        }
        if self.demo {
            self.notify("Design preview. Switch to your workspace in Settings.");
            return;
        }
        match platform::launch(&self.engines[index], None) {
            Ok(()) => self.notify("Starting Unreal Editor…"),
            Err(e) => self.notify(e),
        }
    }
    fn add_engine(&mut self) {
        if let Some(root) = rfd::FileDialog::new()
            .set_title("Select an Unreal Engine installation folder")
            .pick_folder()
        {
            if let Some(e) = platform::inspect_engine(&root, "") {
                if self.demo {
                    self.demo = false;
                    self.engines.clear();
                    self.projects = self.prefs.projects.clone();
                }
                if !self.engines.iter().any(|x| x.path == e.path) {
                    if !self.prefs.engine_paths.contains(&e.path) {
                        self.prefs.engine_paths.push(e.path.clone());
                    }
                    self.engines.push(e);
                }
                self.persist();
                self.notify("Engine folder added to your workspace.");
            } else {
                self.notify("No Unreal Editor and Build.version found in this folder.");
            }
        }
    }
    fn layout(&mut self, ui: &mut Ui, ctx: &Context) {
        let size = ui.max_rect().size();
        let c = Canvas {
            origin: ui.max_rect().min,
            sx: 1.,
            sy: 1.,
            width: size.x,
            height: size.y,
            theme: self.theme.clone(),
        };
        c.fill(ui, [0., 0., c.width, c.height], self.theme.background, 0.);
        let side = 240.;
        c.fill(ui, [0., 0., side, c.height], self.theme.sidebar, 0.);
        c.line(ui, [side, 0.], [side, c.height], self.theme.border);
        let brand = &self.textures[if self.prefs.theme == "light" {
            "brand-light"
        } else {
            "brand"
        }];
        let size = brand.size_vec2();
        let scale = (208. / size.x).min(76. / size.y);
        let width = size.x * scale;
        let height = size.y * scale;
        c.image(
            ui,
            brand,
            [(side - width) / 2., 54. - height / 2., width, height],
            [255, 255, 255],
        );
        let free_offers_y = c.height - 265.;
        let launcher_update_y = if self.prefs.show_free_offers_button {
            free_offers_y - 46.
        } else {
            c.height - 251.
        };
        // Reserve space for the update notice above the bottom-anchored offer
        // button, including at the smallest supported window height.
        let nav_step = if self.launcher_updates.available.is_some() {
            let last_index = if self.prefs.show_vault_cache_button {
                4.
            } else {
                3.
            };
            ((launcher_update_y - 8. - 48. - 111.) / last_index).clamp(50., 58.)
        } else {
            58.
        };
        for (page, label, icon, y) in [
            (Page::Workspace, "Workspace", Icon::Home, 111.),
            (Page::Projects, "Projects", Icon::Folder, 111. + nav_step),
            (Page::Library, "Library", Icon::Cube, 111. + 2. * nav_step),
            (
                Page::Downloads,
                "Downloads",
                Icon::Download,
                111. + 3. * nav_step,
            ),
            (
                Page::VaultCache,
                "Vault Cache",
                Icon::Database,
                111. + 4. * nav_step,
            ),
            (
                Page::FreeOffers,
                "Limited-Time Free",
                Icon::Layers,
                free_offers_y,
            ),
        ] {
            if page == Page::FreeOffers && !self.prefs.show_free_offers_button {
                continue;
            }
            if page == Page::VaultCache && !self.prefs.show_vault_cache_button {
                continue;
            }
            let active = self.page == page;
            if active {
                c.fill(ui, [10., y, side - 20., 48.], SIDEBAR_HIGHLIGHT, 7.);
            }
            let r = ui
                .interact(
                    c.rect([10., y, side - 20., 48.]),
                    Id::new(label),
                    Sense::click(),
                )
                .on_hover_cursor(CursorIcon::PointingHand);
            if r.hovered() && !active {
                c.fill(ui, [10., y, side - 20., 48.], SIDEBAR_HIGHLIGHT, 7.);
            }
            c.icon(
                ui,
                icon,
                34.,
                y + 13.,
                24.,
                if active || r.hovered() {
                    SIDEBAR_ON_HIGHLIGHT
                } else {
                    self.theme.muted
                },
            );
            c.text(
                ui,
                77.,
                if page == Page::FreeOffers && self.free_offer_countdown().is_some() {
                    y + 16.
                } else {
                    y + 24.
                },
                label,
                if page == Page::FreeOffers { 15. } else { 16. },
                false,
                if active || r.hovered() {
                    SIDEBAR_ON_HIGHLIGHT
                } else {
                    self.theme.muted
                },
                Align2::LEFT_CENTER,
            );
            if page == Page::FreeOffers {
                if let Some(countdown) = self.free_offer_countdown() {
                    c.text(
                        ui,
                        77.,
                        y + 35.,
                        &countdown,
                        11.,
                        false,
                        if active || r.hovered() {
                            SIDEBAR_ON_HIGHLIGHT
                        } else {
                            self.theme.muted
                        },
                        Align2::LEFT_CENTER,
                    );
                }
            }
            let update_hint = match page {
                Page::Library if self.has_plugin_updates() => {
                    Some("Plugin updates available for your installed engines")
                }
                _ => None,
            };
            if let Some(update_hint) = update_hint {
                let badge_color = if active || r.hovered() {
                    SIDEBAR_ON_HIGHLIGHT
                } else {
                    self.theme.warning
                };
                let center = c.point(213., y + 24.);
                ui.painter().circle_stroke(
                    center,
                    8. * c.scale(),
                    Stroke::new(1.5, color(badge_color)),
                );
                c.text(
                    ui,
                    213.,
                    y + 24.,
                    "!",
                    13.,
                    true,
                    badge_color,
                    Align2::CENTER_CENTER,
                );
                ui.interact(
                    c.rect([201., y + 12., 24., 24.]),
                    Id::new(("update-badge", label)),
                    Sense::hover(),
                )
                .on_hover_text(update_hint);
            }
            if page == Page::Downloads
                && (self.transfer.busy()
                    || self.plugin_work.mutating
                    || self.library_work_pending())
            {
                let rect = c.rect([200., y + 14., 20., 20.]);
                let mut indicator = ui.new_child(UiBuilder::new().max_rect(rect));
                indicator.put(rect, loading::spinner(&self.theme));
                ui.ctx().request_repaint();
            }
            if r.clone()
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked()
            {
                self.plugin_engine = None;
                self.page = page;
                self.project_scroll = 0.;
            }
        }
        c.line(
            ui,
            [16., c.height - 205.],
            [224., c.height - 205.],
            self.theme.border,
        );
        if self.launcher_updates.available.is_some()
            && c.button(
                ui,
                "launcher-update-available",
                [16., launcher_update_y, 208., 36.],
                "Launcher update available",
                None,
                false,
            )
        {
            self.plugin_engine = None;
            self.page = Page::Settings;
        }
        if c.nav(ui, "Documentation", Icon::Book, c.height - 193., false) {
            let _ = platform::open_external(
                "https://dev.epicgames.com/documentation/en-us/unreal-engine",
            );
        }
        if c.nav(
            ui,
            "Settings",
            Icon::Gear,
            c.height - 139.,
            matches!(self.page, Page::Settings | Page::Engines),
        ) {
            self.plugin_engine = None;
            self.page = Page::Settings;
        }
        c.line(
            ui,
            [16., c.height - 76.],
            [224., c.height - 76.],
            self.theme.border,
        );
        let account_response = ui
            .interact(
                c.rect([18., c.height - 67., 209., 55.]),
                Id::new("account"),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        c.icon(
            ui,
            Icon::User,
            36.,
            c.height - 53.,
            24.,
            if account_response.hovered() {
                [255, 255, 255]
            } else {
                self.theme.muted
            },
        );
        c.text(
            ui,
            83.,
            c.height - 41.,
            if self.demo {
                "Revoxis"
            } else if let Some(s) = &self.session {
                &s.display_name
            } else {
                "Sign in to Epic"
            },
            15.,
            false,
            if account_response.hovered() {
                [255, 255, 255]
            } else {
                self.theme.muted
            },
            Align2::LEFT_CENTER,
        );
        c.icon(
            ui,
            Icon::Down,
            207.,
            c.height - 46.,
            10.,
            if account_response.hovered() {
                [255, 255, 255]
            } else {
                self.theme.muted
            },
        );
        Popup::menu(&account_response)
            .frame(menu_frame(&self.theme))
            .show(|ui| {
                style_menu(ui, &self.theme);
                ui.set_min_width(170.);
                if self.session.is_some() {
                    if ui
                        .add_enabled(
                            self.pending.is_none() && !self.transfer.preparing(),
                            buttons::secondary_button("Sign Out"),
                        )
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.sign_out_account();
                        ui.close();
                    }
                } else if ui
                    .add(buttons::secondary_button("Sign in to Epic"))
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.plugin_engine = None;
                    self.page = Page::Settings;
                    self.login = true;
                    ui.close();
                }
            });
        // Native window controls. The unused top strip acts as a drag region.
        let drag = ui.interact(
            c.rect([245., 0., c.width - 346., 24.]),
            Id::new("title-drag"),
            Sense::drag(),
        );
        if drag.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if c.window_button(ui, c.width - 87., "−") {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
        if c.window_button(ui, c.width - 54., "□") {
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
        if c.window_button(ui, c.width - 21., "×") {
            self.persist();
            if !crate::window_frame::close_button(self.prefs.close_to_tray) {
                self.notify("Could not create the tray icon. The launcher is still open.");
            }
        }
        let header = PageHeaderLayout::new();
        let (title, subtitle) = match self.page {
            Page::Workspace => ("Workspace", "Everything you need to build."),
            Page::Projects => ("Projects", "Your work. Ready when you are."),
            Page::Library => ("Library", "Your assets, projects and plugins from Fab."),
            Page::FreeOffers => ("Limited-Time Free", "What Fab is giving away right now."),
            Page::VaultCache => (
                "Vault Cache",
                "Your cached downloads and the space they use.",
            ),
            Page::Engines => (
                "Engine versions",
                "Releases, hotfixes and your installed engines.",
            ),
            Page::Downloads => {
                if self.downloads.settings {
                    (
                        "Download settings",
                        "Control how Unreal Engine content is installed.",
                    )
                } else {
                    (
                        "Downloads",
                        "Manage engine, plugin and asset installations.",
                    )
                }
            }
            Page::Settings => ("Settings", "Make this workspace yours."),
        };
        c.text(
            ui,
            271.,
            header.title_y,
            title,
            34.,
            true,
            self.theme.text,
            Align2::LEFT_CENTER,
        );
        let actions_left = c.width
            - match self.page {
                Page::Workspace | Page::Projects => 535.,
                Page::Engines => 412.,
                Page::Library => 350.,
                Page::FreeOffers | Page::VaultCache => 0.,
                Page::Downloads => 246.,
                Page::Settings => 236.,
            };
        let mut subtitle_job = egui::text::LayoutJob::simple(
            subtitle.to_owned(),
            FontId::proportional(18. * c.scale()),
            color(self.theme.muted),
            (actions_left - 283.).max(1.) * c.sx,
        );
        subtitle_job.wrap.max_rows = 1;
        let subtitle_galley = ui.painter().layout_job(subtitle_job);
        ui.painter().galley(
            c.point(271., header.subtitle_y) - vec2(0., subtitle_galley.size().y / 2.),
            subtitle_galley,
            color(self.theme.muted),
        );
        ui.interact(
            c.rect([
                271.,
                header.subtitle_y - 12.,
                (actions_left - 283.).max(1.),
                24.,
            ]),
            Id::new("page-subtitle"),
            Sense::hover(),
        )
        .on_hover_text(subtitle);
        if self.page == Page::Settings {
            if c.button(
                ui,
                "settings-engine-versions",
                button_slot([c.width - 206., header.actions_y, 180., 31.]),
                "Engine versions",
                Some(Icon::Chip),
                false,
            ) {
                self.page = Page::Engines;
            }
            if self.has_engine_updates() {
                let x = c.width - 224.;
                let y = header.actions_y + 15.5;
                ui.painter().circle_stroke(
                    c.point(x, y),
                    8. * c.scale(),
                    Stroke::new(1.5, color(self.theme.warning)),
                );
                c.text(
                    ui,
                    x,
                    y,
                    "!",
                    13.,
                    true,
                    self.theme.warning,
                    Align2::CENTER_CENTER,
                );
                ui.interact(
                    c.rect([x - 12., y - 12., 24., 24.]),
                    Id::new("settings-engine-update-badge"),
                    Sense::hover(),
                )
                .on_hover_text("Engine updates available for your installed engines");
            }
        }
        if matches!(self.page, Page::Workspace | Page::Projects) {
            if c.button(
                ui,
                "find-projects",
                button_slot([c.width - 535., header.actions_y, 150., 31.]),
                "Find projects",
                Some(Icon::Search),
                false,
            ) {
                self.open_project_finder();
            }
            if c.button(
                ui,
                "import",
                button_slot([c.width - 373., header.actions_y, 160., 31.]),
                "Import project",
                Some(Icon::Upload),
                false,
            ) {
                self.import(ctx);
            }
            self.launch_button(
                ui,
                &c,
                button_slot([c.width - 203., header.actions_y, 177., 31.]),
            );
        }
        if self.page != Page::Downloads {
            c.line(
                ui,
                [271., header.divider_y],
                [c.width - 26., header.divider_y],
                self.theme.border,
            );
        }
        match self.page {
            Page::Workspace => {
                self.workspace_body(ui, &c, ctx);
            }
            Page::Projects => {
                self.project_section(ui, &c, ctx, header.divider_y);
            }
            Page::Library => self.library_page(ui, &c),
            Page::FreeOffers => self.free_offers_page(ui, &c),
            Page::VaultCache => self.vault_cache_page(ui, &c),
            Page::Engines => self.release_page(ui, &c, ctx),
            Page::Downloads => self.downloads_page(ui, &c),
            Page::Settings => self.settings_page(ui, &c, ctx),
        }
        let status_color = if self.error.is_some() {
            self.theme.warning
        } else if self.snapshot.verified() && !self.demo {
            self.theme.success
        } else {
            self.theme.subtle
        };
        let storage = if self.demo {
            "Storage 248 GB available".into()
        } else {
            self.free
                .map(|n| format!("Storage {} GB available", n / 1_073_741_824))
                .unwrap_or_else(|| "Storage unavailable".into())
        };
        let storage = if self.page == Page::Library && !self.library.footer.is_empty() {
            format!("{} | {storage}", self.library.footer)
        } else {
            storage
        };
        let activity = if self.backup.busy() {
            Some((
                "Project backup".into(),
                self.backup.status(),
                self.backup.fraction(),
            ))
        } else if self.transfer.busy() {
            let p = self.transfer.operation.progress();
            Some((
                self.transfer.state.clone(),
                p.phase,
                if p.total > 0 {
                    Some((p.completed as f32 / p.total as f32).clamp(0., 1.))
                } else {
                    None
                },
            ))
        } else if let Some(status) = self.library_transfer_status() {
            Some(("Library transfer".into(), status.into(), None))
        } else if self.plugin_work.mutating {
            Some((
                self.plugin_work
                    .snapshot()
                    .map(|p| p.title)
                    .unwrap_or_else(|| "Plugin operation".into()),
                self.plugin_work.status.clone(),
                self.plugin_work.fraction(),
            ))
        } else {
            None
        };
        if let Some((title, detail, progress)) = activity {
            let rect = c.rect([271., c.height - 47., (c.width - 540.).max(220.), 42.]);
            ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                ui.set_width(rect.width());
                ui.spacing_mut().item_spacing.y = 3.;
                let text = match progress {
                    Some(p) => format!("{title} - {detail} · {:.0}%", p * 100.),
                    None => format!("{title} - {detail}"),
                };
                ui.add(
                    Label::new(
                        RichText::new(text)
                            .size(13. * c.scale())
                            .color(color(self.theme.text)),
                    )
                    .truncate(),
                )
                .on_hover_text(&detail);
                if let Some(value) = progress {
                    ui.add(
                        ProgressBar::new(value)
                            .desired_width(rect.width())
                            .desired_height(5.),
                    );
                } else {
                    indeterminate_progress(
                        ui,
                        rect.width(),
                        5.,
                        self.theme.accent,
                        self.theme.border,
                    );
                }
            });
            if ui
                .interact(rect, Id::new("footer-transfer-progress"), Sense::click())
                .on_hover_text(if self.backup.busy() {
                    "View project backup"
                } else {
                    "Open Downloads"
                })
                .clone()
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked()
            {
                if self.backup.busy() {
                    self.backup.open = true;
                } else {
                    self.page = Page::Downloads;
                    self.plugin_engine = None;
                }
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            if self.page != Page::Library {
                c.circle(ui, 280., c.height - 26., 6., status_color);
            }
            let status = if self.page == Page::Downloads {
                "No active downloads".into()
            } else if self.page == Page::Library {
                self.library_footer_status().to_owned()
            } else if self.page == Page::Engines {
                if let Some(error) = &self.error {
                    error.clone()
                } else if self.pending.is_some() {
                    self.status.clone()
                } else if self.snapshot.verified() {
                    format!(
                        "Verified against Epic • {} catalog branches • {} account-visible builds • hotfix tracking active",
                        self.snapshot.catalog_count,
                        self.snapshot.releases.len()
                    )
                } else {
                    "Sign in with Epic to verify hotfixes and account-visible builds.".into()
                }
            } else {
                self.status.clone()
            };
            if self.page == Page::Library {
                if !status.is_empty() {
                    ui.put(
                        c.rect([271., c.height - 37., 22., 22.]),
                        loading::spinner(&self.theme),
                    );
                    let storage_width = ui
                        .painter()
                        .layout_no_wrap(
                            storage.clone(),
                            FontId::proportional(14. * c.scale()),
                            color(self.theme.muted),
                        )
                        .size()
                        .x;
                    let left = c.point(305., c.height - 40.);
                    let right = c.point(c.width - 31., c.height - 12.);
                    let rect = Rect::from_min_max(
                        left,
                        pos2((right.x - storage_width - 24.).max(left.x + 1.), right.y),
                    );
                    ui.scope_builder(
                        UiBuilder::new()
                            .max_rect(rect)
                            .layout(Layout::left_to_right(Align::Center)),
                        |ui| {
                            ui.add(
                                Label::new(
                                    RichText::new(&status)
                                        .size(14. * c.scale())
                                        .color(color(self.theme.text)),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&status);
                        },
                    );
                }
            } else {
                let storage_width = ui
                    .painter()
                    .layout_no_wrap(
                        storage.clone(),
                        FontId::proportional(14. * c.scale()),
                        color(self.theme.muted),
                    )
                    .size()
                    .x;
                let left = c.point(297., c.height - 40.);
                let right = c.point(c.width - 31., c.height - 12.);
                let rect = Rect::from_min_max(
                    left,
                    pos2((right.x - storage_width - 24.).max(left.x + 1.), right.y),
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(rect)
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        ui.add(
                            Label::new(
                                RichText::new(&status)
                                    .size(14. * c.scale())
                                    .color(color(self.theme.muted)),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&status);
                    },
                );
            }
        }
        c.text(
            ui,
            c.width - 31.,
            c.height - 26.,
            &storage,
            14.,
            false,
            self.theme.muted,
            Align2::RIGHT_CENTER,
        );
        if let Some(toast) = &self.toast {
            if toast.created.elapsed() < Duration::from_secs(6) {
                let width = ((c.width - 340.) * c.sx).min(1000.);
                let galley = ui.painter().layout(
                    toast.message.clone(),
                    FontId::proportional(15. * c.scale()),
                    color(self.theme.launch_foreground),
                    width - 32.,
                );
                let size = vec2(width, galley.size().y + 28.);
                let center = c.point((271. + c.width - 26.) / 2., c.height - 82.);
                let rect = Rect::from_center_size(center, size);
                ui.painter()
                    .rect_filled(rect, 7., color(self.theme.launch_background));
                ui.painter().galley(
                    rect.min + vec2(16., 14.),
                    galley,
                    color(self.theme.launch_foreground),
                );
                if toast.scan_results
                    && ui
                        .interact(rect, Id::new("scan-result-toast"), Sense::click())
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(
                            "View the latest scan results and skipped folder paths in Downloads.",
                        )
                        .clicked()
                {
                    self.page = Page::Downloads;
                    self.downloads.settings = false;
                    self.downloads.show_overview();
                    self.toast = None;
                }
            }
        }
    }
    fn workspace_body(&mut self, ui: &mut Ui, c: &Canvas, ctx: &Context) {
        let columns = card_columns(c.width - 297., ENGINE_CARD_WIDTH, ENGINE_CARD_GAP);
        let rows = (self.engines.len() + self.prefs.pending_engines.len())
            .div_ceil(columns)
            .max(1);
        let extra = (rows - 1) as f32 * (ENGINE_CARD_HEIGHT + ENGINE_CARD_GAP);
        let top = PageHeaderLayout::new().divider_y + 1.;
        let viewport = c.rect([271., top, c.width - 277., (c.height - top - 50.).max(1.)]);
        let theme = self.theme.clone();
        ui.scope_builder(UiBuilder::new().max_rect(viewport), |ui| {
            AppScrollArea::vertical(&theme)
                .id_salt("workspace-body")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let local = Canvas {
                        origin: pos2(c.origin.x, ui.cursor().top() - top),
                        width: c.width,
                        height: c.height + extra,
                        sx: c.sx,
                        sy: c.sy,
                        theme: c.theme.clone(),
                    };
                    ui.set_min_height(viewport.height() + extra);
                    let divider = self.engine_section(ui, &local, c.height < 800.);
                    self.project_section(ui, &local, ctx, divider);
                    if DragAndDrop::payload::<engine_order::EngineCardDrag>(ui.ctx()).is_some() {
                        if let Some(pointer) = ui
                            .ctx()
                            .pointer_latest_pos()
                            .filter(|p| viewport.contains(*p))
                        {
                            let direction = if pointer.y < viewport.top() + 32. {
                                1.
                            } else if pointer.y > viewport.bottom() - 32. {
                                -1.
                            } else {
                                0.
                            };
                            ui.scroll_with_delta(vec2(
                                0.,
                                direction * 480. * ui.input(|i| i.stable_dt).min(0.05),
                            ));
                        }
                        ui.ctx().request_repaint();
                    }
                });
        });
        engine_order::paint_drag_label(ctx, &self.engines, &self.theme);
    }
    fn engine_section(&mut self, ui: &mut Ui, c: &Canvas, short: bool) -> f32 {
        let offset = PageHeaderLayout::new().divider_y
            - PAGE_HEADER_DIVIDER_Y
            - if short { 16. } else { 0. };
        let local = Canvas {
            origin: c.origin + vec2(0., offset),
            width: c.width,
            height: c.height,
            sx: 1.,
            sy: 1.,
            theme: c.theme.clone(),
        };
        let c = &local;
        let columns = card_columns(c.width - 297., ENGINE_CARD_WIDTH, ENGINE_CARD_GAP);
        let rows = (self.engines.len() + self.prefs.pending_engines.len())
            .div_ceil(columns)
            .max(1);
        let divider = ENGINE_CARDS_TOP + rows as f32 * (ENGINE_CARD_HEIGHT + ENGINE_CARD_GAP)
            - ENGINE_CARD_GAP
            + if short { 12. } else { 24. };
        c.line(
            ui,
            [271., divider],
            [c.width - 26., divider],
            self.theme.border,
        );
        c.icon(ui, Icon::Chip, 271., 132., 24., self.theme.muted);
        self.engine_storage_badge(ui, c);
        c.text(
            ui,
            309.,
            144.,
            "Engine versions",
            21.,
            true,
            self.theme.text,
            Align2::LEFT_CENTER,
        );
        if c.button(
            ui,
            "install",
            button_slot([c.width - 156., 126., 130., 31.]),
            "Install engine",
            Some(Icon::Plus),
            false,
        ) {
            self.prefs.pending_engines.push(String::new());
            self.persist();
        }
        if self.engines.is_empty() && self.prefs.pending_engines.is_empty() {
            c.text(
                ui,
                271.,
                214.,
                "No installed engines found",
                21.,
                true,
                self.theme.text,
                Align2::LEFT_CENTER,
            );
            c.text(
                ui,
                271.,
                249.,
                "Locate an engine installation, or view available releases.",
                16.,
                false,
                self.theme.muted,
                Align2::LEFT_CENTER,
            );
            if c.button(
                ui,
                "addexisting",
                [271., 263., 300., 40.],
                "Locate an installed engine",
                Some(Icon::Folder),
                false,
            ) {
                self.add_engine();
            }
        }
        let order = engine_order::indices(&self.engines, &self.prefs.engine_card_order);
        for position in 0..self.engines.len() + self.prefs.pending_engines.len() {
            let slot = position % columns;
            let row = position / columns;
            let i = order.get(position).copied().unwrap_or(position);
            if i >= self.engines.len() {
                let draft = i - self.engines.len();
                if draft >= self.prefs.pending_engines.len() {
                    break;
                }
                self.pending_engine_card(
                    ui,
                    c,
                    draft,
                    [
                        271. + slot as f32 * (ENGINE_CARD_WIDTH + ENGINE_CARD_GAP),
                        ENGINE_CARDS_TOP + row as f32 * (ENGINE_CARD_HEIGHT + ENGINE_CARD_GAP),
                        ENGINE_CARD_WIDTH,
                        ENGINE_CARD_HEIGHT,
                    ],
                );
                continue;
            }
            self.engine_card(
                ui,
                c,
                i,
                [
                    271. + slot as f32 * (ENGINE_CARD_WIDTH + ENGINE_CARD_GAP),
                    ENGINE_CARDS_TOP + row as f32 * (ENGINE_CARD_HEIGHT + ENGINE_CARD_GAP),
                    ENGINE_CARD_WIDTH,
                    ENGINE_CARD_HEIGHT,
                ],
            );
        }
        divider + offset
    }
    fn project_toolbar(&mut self, ui: &mut Ui, c: &Canvas, right: f32, top: f32, compact: bool) {
        let search_w = if compact { 145. } else { 258. };
        let filter_w = if compact { 110. } else { 130. };
        let sort_w = 170.;
        let list = right - 32.;
        let grid = list - 40.;
        let sort = grid - 8. - sort_w;
        let filter = sort - 8. - filter_w;
        let search = filter - 8. - search_w;
        let refresh_rect = [search - 40., top, 32., 32.];
        let manage_w = if compact { 32. } else { 152. };
        if c.button(
            ui,
            ("manage-project-categories", compact),
            button_slot([refresh_rect[0] - manage_w - 8., top, manage_w, 32.]),
            if compact { "" } else { "Manage categories" },
            if compact { Some(Icon::Folder) } else { None },
            false,
        ) {
            self.category_manager = true;
        }
        ui.interact(
            c.rect([refresh_rect[0] - manage_w - 8., top, manage_w, 32.]),
            Id::new(("manage-categories-tip", compact)),
            Sense::hover(),
        )
        .on_hover_text("Manage project categories");
        if self.scan.receiver.is_some() {
            ui.put(c.rect(refresh_rect), loading::spinner(&self.theme))
                .on_hover_text("Refreshing configured project folders…");
        } else {
            if c.icon_button(
                ui,
                ("refresh-projects", compact),
                button_slot(refresh_rect),
                Icon::Reset,
                false,
            ) {
                self.project_scroll = 0.;
                self.start_scan(ui.ctx().clone(), None);
            }
            ui.interact(
                c.rect(refresh_rect),
                Id::new(("refresh-projects-tip", compact)),
                Sense::hover(),
            )
            .on_hover_text("Refresh projects from configured folders and remove missing entries");
        }
        ui.scope_builder(
            UiBuilder::new().max_rect(c.rect([search, top, search_w, 32.])),
            |ui| {
                if search_field(
                    ui,
                    &mut self.search,
                    "Search projects…",
                    search_w * c.sx,
                    &self.theme,
                    c.scale(),
                )
                .changed()
                {
                    self.project_scroll = 0.;
                }
            },
        );
        ui.scope_builder(
            UiBuilder::new().max_rect(c.rect([filter, top, filter_w, 32.])),
            |ui| {
                ui.style_mut().override_font_id = Some(FontId::proportional(14. * c.scale()));
                ui.spacing_mut().button_padding = vec2(10., 5.) * c.scale();
                ui.spacing_mut().interact_size.y = 32. * c.scale();
                AppComboBox::new(&self.theme, ("project-version-filter", compact))
                    .width(filter_w * c.sx)
                    .selected_text(if self.filter.trim().is_empty() {
                        "Unknown engine"
                    } else {
                        &self.filter
                    })
                    .show_ui(ui, |ui| {
                        if buttons::selectable_value(
                            ui,
                            &mut self.filter,
                            "All versions".into(),
                            "All versions",
                        )
                        .clicked()
                        {
                            self.project_scroll = 0.;
                        }
                        let mut versions: Vec<_> = self
                            .projects
                            .iter()
                            .filter(|p| self.project_visible(&p.path))
                            .map(|p| p.engine.clone())
                            .collect();
                        versions.sort_by(|a, b| {
                            EngineVersion::parse(b)
                                .cmp(&EngineVersion::parse(a))
                                .then_with(|| a.cmp(b))
                        });
                        versions.dedup();
                        for version in versions {
                            let label = if version.trim().is_empty() {
                                "Unknown engine"
                            } else {
                                &version
                            };
                            if buttons::selectable_value(
                                ui,
                                &mut self.filter,
                                version.clone(),
                                label,
                            )
                            .clicked()
                            {
                                self.project_scroll = 0.;
                            }
                        }
                    });
            },
        );
        ui.scope_builder(
            UiBuilder::new().max_rect(c.rect([sort, top, sort_w, 32.])),
            |ui| {
                ui.style_mut().override_font_id = Some(FontId::proportional(14. * c.scale()));
                ui.spacing_mut().button_padding = vec2(10., 5.) * c.scale();
                ui.spacing_mut().interact_size.y = 32. * c.scale();
                AppComboBox::new(&self.theme, ("project-sort", compact))
                    .width(sort_w * c.sx)
                    .selected_text(self.project_sort.label())
                    .show_ui(ui, |ui| {
                        for sort in [
                            ProjectSort::RecentlyOpened,
                            ProjectSort::NameAscending,
                            ProjectSort::NameDescending,
                        ] {
                            if buttons::selectable_value(
                                ui,
                                &mut self.project_sort,
                                sort,
                                sort.label(),
                            )
                            .changed()
                            {
                                self.project_scroll = 0.;
                            }
                        }
                    });
            },
        );
        if c.icon_button(
            ui,
            ("project-grid", compact),
            button_slot([grid, top, 32., 32.]),
            Icon::Grid,
            !self.list_view,
        ) {
            self.list_view = false;
        }
        if c.icon_button(
            ui,
            ("project-list", compact),
            button_slot([list, top, 32., 32.]),
            Icon::List,
            self.list_view,
        ) {
            self.list_view = true;
        }
    }
    fn project_section(&mut self, ui: &mut Ui, c: &Canvas, ctx: &Context, divider_y: f32) {
        let y = divider_y + FILTER_DIVIDER_GAP;
        let wrap = c.width < 1320.;
        let extra = if wrap {
            FILTER_ROW_HEIGHT + FILTER_ROW_GAP
        } else {
            0.
        };
        let bottom = c.height - 50.;
        let categories_y = y + FILTER_ROW_HEIGHT + FILTER_ROW_GAP + extra;
        let content_top = categories_y + FILTER_ROW_HEIGHT + FILTER_ROW_GAP;
        let track_x = c.width - 26. - scrollbar::MARGIN - scrollbar::WIDTH;
        let grid_width = track_x - scrollbar::GRID_MARGIN - 271.;
        let (columns, card_width) =
            card_grid_metrics(grid_width, PROJECT_CARD_WIDTH, PROJECT_CARD_GAP);
        let card_height = card_width - 2. + 64.;
        c.text(
            ui,
            271.,
            y + FILTER_ROW_HEIGHT / 2.,
            &format!(
                "Projects ({})",
                self.projects
                    .iter()
                    .filter(|p| self.project_visible(&p.path))
                    .count()
            ),
            23.,
            true,
            self.theme.text,
            Align2::LEFT_CENTER,
        );
        self.project_toolbar(
            ui,
            c,
            c.width - 26.,
            y + extra + (FILTER_ROW_HEIGHT - 32.) / 2.,
            wrap,
        );
        self.category_tabs(ui, c, categories_y);
        let mut indices: Vec<_> = (0..self.projects.len())
            .filter(|i| {
                let p = &self.projects[*i];
                self.project_visible(&p.path)
                    && (!self.pinned_only || p.pinned)
                    && self.category_filter.as_ref().is_none_or(|category| {
                        self.prefs
                            .project_category_assignments
                            .get(&p.path)
                            .map(String::as_str)
                            .unwrap_or("")
                            == category
                    })
                    && (self.filter == "All versions" || p.engine == self.filter)
                    && p.name.to_lowercase().contains(&self.search.to_lowercase())
            })
            .collect();
        match self.project_sort {
            ProjectSort::NameAscending => {
                indices.sort_by_key(|i| self.projects[*i].name.to_lowercase());
            }
            ProjectSort::NameDescending => {
                indices.sort_by_key(|i| std::cmp::Reverse(self.projects[*i].name.to_lowercase()));
            }
            ProjectSort::RecentlyOpened if !self.demo => {
                indices.sort_by_key(|i| std::cmp::Reverse(self.projects[*i].last_opened));
            }
            ProjectSort::RecentlyOpened => {}
        }
        let content = c.rect([
            271.,
            content_top,
            grid_width,
            (bottom - content_top).max(1.),
        ]);
        if indices.is_empty() {
            let empty_y = (content_top + (bottom - content_top) / 2. - 48.).max(content_top + 16.);
            c.text(
                ui,
                (271. + c.width - 26.) / 2.,
                empty_y,
                if self.projects.is_empty() {
                    "Find your Unreal projects"
                } else {
                    "No matching projects"
                },
                27.,
                true,
                self.theme.text,
                Align2::CENTER_CENTER,
            );
            c.text(
                ui,
                (271. + c.width - 26.) / 2.,
                empty_y + 42.,
                if self.projects.is_empty() {
                    "Add a folder with one or many projects. Subfolders are included."
                } else {
                    "Try another search, engine version or category."
                },
                17.,
                false,
                self.theme.muted,
                Align2::CENTER_CENTER,
            );
            if self.projects.is_empty() {
                let center = (271. + c.width - 26.) / 2.;
                if c.button(
                    ui,
                    "empty-find-projects",
                    [center - 166., empty_y + 76., 156., 32.],
                    "Find projects",
                    Some(Icon::Search),
                    true,
                ) {
                    self.open_project_finder();
                }
                if c.button(
                    ui,
                    "empty-project-folders",
                    [center + 6., empty_y + 76., 160., 32.],
                    "Add project folder",
                    Some(Icon::Folder),
                    false,
                ) {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("Choose a folder containing one or more Unreal projects")
                        .pick_folder()
                    {
                        self.add_project_root(path, ctx);
                    }
                }
            }
            return;
        }
        let row_h = if self.list_view {
            86.
        } else {
            card_height + PROJECT_CARD_GAP
        };
        let rows = if self.list_view {
            indices.len()
        } else {
            indices.len().div_ceil(columns)
        };
        let max_scroll = (rows as f32 * row_h - (bottom - content_top).max(1.)).max(0.);
        if ui.rect_contains_pointer(content) {
            let previous = self.project_scroll;
            self.project_scroll = (self.project_scroll
                - ui.input(|i| i.smooth_scroll_delta.y) / c.sy)
                .clamp(0., max_scroll);
            if self.page == Page::Workspace && self.project_scroll != previous {
                // Let the outer workspace scroll only when the project list
                // reaches an edge, rather than moving both views at once.
                ui.input_mut(|i| i.smooth_scroll_delta.y = 0.);
            }
        }
        self.project_scroll = self.project_scroll.min(max_scroll);
        if max_scroll > 0. {
            let track = c.rect([
                track_x,
                content_top,
                scrollbar::WIDTH,
                (bottom - content_top).max(1.),
            ]);
            let visible = (bottom - content_top).max(1.);
            let thumb_height =
                scrollbar::thumb_length(track.height(), visible, rows as f32 * row_h);
            let travel = (track.height() - thumb_height).max(1.);
            let thumb = Rect::from_min_size(
                pos2(
                    track.min.x,
                    track.min.y + travel * self.project_scroll / max_scroll,
                ),
                vec2(track.width(), thumb_height),
            );
            let track_response =
                ui.interact(track, Id::new("project-scroll-track"), Sense::click());
            let thumb_response = ui.interact(
                thumb,
                Id::new("project-scroll-thumb"),
                Sense::click_and_drag(),
            );
            if thumb_response.dragged() {
                self.project_scroll = (self.project_scroll
                    + thumb_response.drag_delta().y / travel * max_scroll)
                    .clamp(0., max_scroll);
            } else if track_response
                .clone()
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked()
            {
                if let Some(p) = track_response.interact_pointer_pos() {
                    self.project_scroll = ((p.y - track.min.y - thumb_height / 2.) / travel
                        * max_scroll)
                        .clamp(0., max_scroll);
                }
            }
            ui.painter().rect_filled(track, 4., color(self.theme.card));
            let thumb = Rect::from_min_size(
                pos2(
                    track.min.x,
                    track.min.y + travel * self.project_scroll / max_scroll,
                ),
                vec2(track.width(), thumb_height),
            );
            ui.painter().rect_filled(
                thumb,
                4.,
                color(if thumb_response.hovered() || thumb_response.dragged() {
                    self.theme.text
                } else {
                    self.theme.muted
                }),
            );
        }
        let mut child = ui.new_child(UiBuilder::new().max_rect(content));
        child.set_clip_rect(content.intersect(ui.clip_rect()));
        for (position, index) in indices.into_iter().enumerate() {
            let p = self.projects[index].clone();
            let (x, cy) = if self.list_view {
                (
                    271.,
                    content_top + 2. + position as f32 * row_h - self.project_scroll,
                )
            } else {
                (
                    271. + (position % columns) as f32 * (card_width + PROJECT_CARD_GAP),
                    content_top + 2. + (position / columns) as f32 * row_h - self.project_scroll,
                )
            };
            if cy + row_h < content_top || cy > bottom {
                continue;
            }
            if self.list_view {
                c.boxed(
                    &child,
                    [x, cy, c.width - 314., 74.],
                    self.theme.card,
                    self.theme.border,
                    5.,
                    1.,
                );
                let row = child
                    .interact(
                        c.rect([x, cy, c.width - 314., 74.]),
                        Id::new(("project-list", index)),
                        Sense::click(),
                    )
                    .on_hover_cursor(CursorIcon::PointingHand);
                if row
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                    || row.secondary_clicked()
                {
                    self.selected = Some(index);
                }
                if row.double_clicked() {
                    self.open_project(index);
                }
                Popup::context_menu(&row)
                    .frame(menu_frame(&self.theme))
                    .show(|ui| self.project_options(ui, index));
                let image = c.rect([x + 9., cy + 9., 56., 56.]);
                if let Some(tex) = p.artwork.as_ref().and_then(|a| self.textures.get(a)) {
                    let size = tex.size_vec2();
                    let (uw, uh) = if size.x > size.y {
                        (size.y / size.x, 1.)
                    } else {
                        (1., size.x / size.y)
                    };
                    let uv = Rect::from_min_max(pos2((1. - uw) / 2., 0.), pos2((1. + uw) / 2., uh));
                    paint_thumbnail(
                        &child,
                        tex,
                        image,
                        uv,
                        CornerRadius::same((4. * c.scale()).round() as u8),
                    );
                } else {
                    c.fill(
                        &child,
                        [x + 9., cy + 9., 56., 56.],
                        self.theme.selection,
                        4.,
                    );
                    c.icon(
                        &child,
                        Icon::Folder,
                        x + 21.,
                        cy + 21.,
                        32.,
                        self.theme.muted,
                    );
                }
                let button_x = c.width - 43. - 14. - 128.;
                let version_x = button_x - 18.;
                let title_clip = c.rect([x + 80., cy, (version_x - 90. - x - 80.).max(0.), 74.]);
                child
                    .painter()
                    .with_clip_rect(title_clip.intersect(child.clip_rect()))
                    .text(
                        c.point(x + 80., cy + 37.),
                        Align2::LEFT_CENTER,
                        &p.name,
                        FontId::new(17. * c.scale(), FontFamily::Name("semibold".into())),
                        color(self.theme.text),
                    );
                let version = EngineVersion::parse(&p.engine)
                    .map(|v| v.branch())
                    .unwrap_or_else(|| "Custom".into());
                c.text(
                    &child,
                    version_x,
                    cy + 37.,
                    &version,
                    16.,
                    false,
                    self.theme.muted,
                    Align2::RIGHT_CENTER,
                );
                if c.button(
                    &mut child,
                    ("open-project-row", index),
                    button_slot([button_x, cy + 21., 128., 32.]),
                    "Open Project",
                    None,
                    true,
                ) {
                    self.open_project(index);
                }
                continue;
            }
            self.project_card(&mut child, c, index, [x, cy, card_width, card_height]);
        }
        let _ = ctx;
    }
    fn release_page(&mut self, ui: &mut Ui, c: &Canvas, ctx: &Context) {
        let header = PageHeaderLayout::new();
        let actions_y = header.actions_y;
        c.text(
            ui,
            271.,
            header.divider_y + 34.,
            "Available engine releases",
            22.,
            true,
            self.theme.text,
            Align2::LEFT_CENTER,
        );
        if c.button(
            ui,
            "check",
            button_slot([c.width - 26. - 154., actions_y, 154., 32.]),
            if self.pending.is_some() {
                "Checking…"
            } else {
                "Check for updates"
            },
            None,
            true,
        ) {
            self.check(ctx.clone());
        }
        if c.button(
            ui,
            "locate-engine",
            button_slot([c.width - 26. - 154. - 12. - 220., actions_y, 220., 32.]),
            "Locate installed engine",
            Some(Icon::Folder),
            false,
        ) {
            self.add_engine();
        }
        let header_y = header.divider_y + 78.;
        c.text(
            ui,
            281.,
            header_y,
            "VERSION",
            12.,
            true,
            self.theme.subtle,
            Align2::LEFT_CENTER,
        );
        c.text(
            ui,
            480.,
            header_y,
            "BUILD / HOTFIX",
            12.,
            true,
            self.theme.subtle,
            Align2::LEFT_CENTER,
        );
        c.text(
            ui,
            c.width - 340.,
            header_y,
            "STATUS",
            12.,
            true,
            self.theme.subtle,
            Align2::LEFT_CENTER,
        );
        let area = c.rect([
            271.,
            header_y + 24.,
            c.width - 318.,
            (c.height - header_y - 101.).max(100.),
        ]);
        ui.scope_builder(UiBuilder::new().max_rect(area),|ui|{AppScrollArea::vertical(&self.theme).id_salt("release-scroll").show(ui,|ui|{
            for r in self.snapshot.releases.clone(){
                let (rect,_)=ui.allocate_exact_size(vec2(area.width()-15.,65.*c.sy),Sense::hover());
                let p=ui.painter().clone();
                if ui.rect_contains_pointer(rect) {
                    p.rect_filled(rect, 0., table_row_hover_color(&self.theme));
                }
                let font=FontId::proportional(15.*c.scale());let v=r.version.as_ref().map(|v|v.to_string()).unwrap_or_else(||r.app.trim_start_matches("UE_").to_owned());
                p.text(rect.left_center()+vec2(10.*c.sx,0.),Align2::LEFT_CENTER,&v,FontId::new(19.*c.scale(),FontFamily::Name("semibold".into())),color(self.theme.text));
                let build = r.build.as_deref().unwrap_or("Sign in to resolve hotfix version");
                let build_rect = Rect::from_min_max(pos2(c.point(480., 0.).x, rect.top()), pos2(c.point(c.width - 356., 0.).x, rect.bottom()));
                let mut build_ui = ui.new_child(UiBuilder::new().max_rect(build_rect).layout(Layout::left_to_right(Align::Center)));
                build_ui.add(Label::new(RichText::new(build).font(font.clone()).color(color(self.theme.muted))).truncate()).on_hover_text(build);
                let installed=self.engines.iter().find(|e|EngineVersion::parse(&e.version).zip(EngineVersion::parse(&r.app)).is_some_and(|(a,b)|a.major==b.major&&a.minor==b.minor));
                let label=match installed{Some(e)=>if r.version.as_ref()>EngineVersion::parse(&e.version).as_ref(){"Update available"}else{"Installed"},None=>if r.build.is_some(){"Available"}else{"Catalog only"}};
                p.text(pos2(c.point(c.width - 340., 0.).x, rect.center().y),Align2::LEFT_CENTER,label,font,color(if label=="Update available"{self.theme.warning}else if label=="Installed"{self.theme.success}else{self.theme.muted}));
                let b=Rect::from_min_size(pos2(rect.right()-142.*c.sx, rect.center().y-COMPACT_BUTTON_HEIGHT*c.sy/2.),vec2(130.*c.sx,COMPACT_BUTTON_HEIGHT*c.sy));
                let already_installed = transfers::release_is_installed(&r, &self.engines);
                let response = ui.scope_builder(UiBuilder::new().max_rect(b), |ui| {
                    ui.add_enabled(!already_installed, buttons::secondary_button(if already_installed { "Installed" } else { "Install engine" }).min_size(b.size()))
                }).inner;
                if response.on_hover_cursor(CursorIcon::PointingHand).clicked() { self.choose_install(r, ctx.clone()); }
                ui.add_space(5.*c.sy);
            }
            if self.snapshot.releases.is_empty(){ui.label("No successful release check yet. Connect your Epic account to get started.");}
        });});
    }
    fn downloads_page(&mut self, ui: &mut Ui, c: &Canvas) {
        self.downloads_view(ui, c);
    }
    fn settings_page(&mut self, ui: &mut Ui, c: &Canvas, ctx: &Context) {
        self.settings_flow(ui, c, ctx);
    }
    fn sign_out_account(&mut self) {
        if self.library_work_pending() {
            self.notify("Finish or stop the current installation and clear the waiting list before signing out.");
            return;
        }
        match storage::sign_out() {
            Ok(()) => {
                self.session = None;
                self.snapshot = Snapshot::default();
                self.prefs.display_name = "Developer".into();
                self.persist();
                self.status = "Signed out • hotfix checks unavailable".into();
                self.next_check = Instant::now() + Duration::from_secs(900);
            }
            Err(e) => self.notify(e),
        }
    }
    fn dialogs(&mut self, ctx: &Context) {
        self.clone_dialog(ctx);
        self.project_backup_dialog(ctx);
        self.plugin_confirmation(ctx);
        if let Some(engine) = self.engine_remove.clone() {
            let mut open = true;
            let mut confirmed = false;
            dialog::show(
                ctx,
                &self.theme.clone(),
                "remove-engine",
                &format!("Remove Unreal Engine {}", engine.version),
                "Review the installation before removing it.",
                &mut open,
                |ui| {
                    ui.label("This permanently deletes the engine folder, including installed plugins and custom files.");
                    dialog::info(ui, &self.theme, "Installation folder", &engine.path);
                    ui.label("Projects outside this folder are kept. Projects using this engine will show a missing-engine warning.");
                    confirmed = dialog::footer(
                        ui,
                        &self.theme,
                        "Remove engine",
                        !self.transfer.busy(),
                        true,
                    );
                },
            );
            if confirmed {
                self.engine_remove = None;
                self.start_maintenance(engine, true, ctx.clone());
            } else if !open {
                self.engine_remove = None;
            }
        }
        self.engine_options_dialog(ctx);
        if self.login && self.pending.is_none() {
            self.sign_in(ctx.clone());
        }
        if self.new_project {
            let mut open = true;
            let mut created = None;
            dialog::show(
                ctx,
                &self.theme.clone(),
                "new-project",
                "New project",
                "Choose a name and location for your project.",
                &mut open,
                |ui| {
                    ui.label("Project name");
                    ui.add_sized(
                        [ui.available_width(), 40.],
                        TextEdit::singleline(&mut self.new_name),
                    );
                    ui.label("Parent folder");
                    dialog::folder(ui, &mut self.new_path);
                    ui.label("Engine version");
                    AppComboBox::new(&self.theme, "new-engine")
                        .width(ui.available_width())
                        .selected_text(
                            self.engines
                                .get(self.new_engine)
                                .map(|e| e.version.as_str())
                                .unwrap_or("No installed engine"),
                        )
                        .show_ui(ui, |ui| {
                            for (i, e) in self.engines.iter().enumerate() {
                                buttons::selectable_value(ui, &mut self.new_engine, i, &e.version);
                            }
                        });
                    if self.demo {
                        ui.label("Switch to your real workspace in Settings to create projects.");
                    }
                    let preview = Path::new(&self.new_path)
                        .join(&self.new_name)
                        .join(format!("{}.uproject", self.new_name));
                    dialog::info(
                        ui,
                        &self.theme,
                        "New project file",
                        &preview.to_string_lossy(),
                    );
                    if dialog::footer(
                        ui,
                        &self.theme,
                        "Create project",
                        !self.demo && !self.engines.is_empty() && !self.new_path.is_empty(),
                        false,
                    ) {
                        match platform::create_project(
                            Path::new(&self.new_path),
                            &self.new_name,
                            &self.engines[self.new_engine],
                        ) {
                            Ok(p) => created = Some(p),
                            Err(e) => self.error = Some(e),
                        }
                    }
                    if let Some(e) = &self.error {
                        ui.colored_label(color(self.theme.warning), e);
                    }
                },
            );
            if let Some(p) = created {
                self.prefs.manual_projects.push(p.path.clone());
                self.projects.push(p);
                self.persist();
                self.new_project = false;
                self.new_name.clear();
                self.notify("Blank project created.");
            } else if !open {
                self.new_project = false;
            }
        }
    }
}
impl eframe::App for Workspace {
    fn on_exit(&mut self) {
        self.persist();
    }
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let backgrounded = crate::window_frame::is_backgrounded()
            || ctx.input(|i| i.viewport().minimized == Some(true));
        if self.layout_probe.is_none() {
            if let Some(position) = crate::window_frame::normal_position() {
                self.prefs.window_position = Some(position);
            }
        }
        ctx.input(|i| {
            let viewport = i.viewport();
            if viewport.minimized != Some(true) {
                self.prefs.window_maximized = viewport.maximized.unwrap_or(false);
                if !self.prefs.window_maximized {
                    if let Some(rect) = viewport.inner_rect {
                        let size = [rect.width(), rect.height()];
                        if size[0] >= 1000. && size[1] >= 680. {
                            self.prefs.window_size = Some(size);
                        }
                    }
                }
            }
        });
        engine_workspace::download_policy::configure(&self.prefs.downloads);
        if !backgrounded {
            self.run_layout_probe(ctx);
            resize_window_edges(ctx);
        }
        if self.launcher_updates.blocks_work() {
            self.poll_launcher_download();
            if self.launcher_updates.blocks_work() {
                self.show_update_preparation(ctx, backgrounded);
                return;
            }
        }
        self.poll(ctx);
        self.poll_transfers(ctx);
        self.poll_plugins(ctx);
        self.poll_library(ctx);
        self.poll_clone(ctx);
        self.poll_project_backup(ctx);
        self.poll_free_offers(ctx);
        self.poll_vault_cache(ctx);
        self.poll_project_finder();
        self.poll_launcher_updates(ctx);
        if self.launcher_updates.blocks_work() {
            self.show_update_preparation(ctx, backgrounded);
            return;
        }
        // Continue consuming worker results and running scheduled release checks,
        // but do not build invisible widgets, animate spinners or start UI-only scans.
        if !backgrounded {
            self.poll_workspace_cache(ctx);
            self.poll_project_thumbnails(ctx);
            egui::CentralPanel::default()
                .frame(Frame::NONE)
                .show(ctx, |ui| {
                    self.layout(ui, ctx);
                });
            self.plugin_dialog(ctx);
            self.category_dialog(ctx);
            self.dialogs(ctx);
            self.install_dialog(ctx);
            self.project_finder_dialog(ctx);
        }
        if crate::window_frame::take_replace_request() {
            if self.transfer.busy()
                || self.library_work_pending()
                || self.plugin_work.mutating
                || self.backup.busy()
            {
                self.notify("An operation is running. Finish it before switching launcher builds.");
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        let busy = self.launcher_updates.active()
            || self.backup.busy()
            || self.cloning.busy()
            || self.pending.is_some()
            || self.transfer.busy()
            || self.scan.receiver.is_some()
            || self.project_finder.busy()
            || self.library_work_pending()
            || self.plugin_work.mutating;
        ctx.request_repaint_after(idle_frame_interval(backgrounded, busy));
    }
}

fn thumbnail_top_corners(radius: f32) -> CornerRadius {
    CornerRadius {
        nw: radius.round() as u8,
        ne: radius.round() as u8,
        sw: 0,
        se: 0,
    }
}

fn paint_thumbnail(ui: &Ui, texture: &TextureHandle, rect: Rect, uv: Rect, corners: CornerRadius) {
    ui.painter().add(
        egui::epaint::RectShape::filled(rect, corners, Color32::WHITE)
            .with_texture(texture.id(), uv),
    );
}

const ENGINE_CARD_WIDTH: f32 = 340.;
const ENGINE_CARD_HEIGHT: f32 = 140.;
const ENGINE_CARD_GAP: f32 = 12.;
const ENGINE_CARDS_TOP: f32 = 170.;

const SIDEBAR_HIGHLIGHT: [u8; 3] = [234, 189, 93];
fn idle_frame_interval(backgrounded: bool, busy: bool) -> Duration {
    match (backgrounded, busy) {
        (true, true) => Duration::from_secs(1),
        (true, false) => Duration::from_secs(30),
        (false, true) => Duration::from_millis(100),
        (false, false) => Duration::from_secs(5),
    }
}
const SIDEBAR_ON_HIGHLIGHT: [u8; 3] = [0, 0, 0];

struct Canvas {
    origin: Pos2,
    width: f32,
    height: f32,
    sx: f32,
    sy: f32,
    theme: Theme,
}
impl Canvas {
    fn scale(&self) -> f32 {
        self.sx.min(self.sy)
    }
    fn point(&self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(x * self.sx, y * self.sy)
    }
    fn rect(&self, r: [f32; 4]) -> Rect {
        Rect::from_min_size(self.point(r[0], r[1]), vec2(r[2] * self.sx, r[3] * self.sy))
    }
    fn fill(&self, ui: &Ui, r: [f32; 4], col: [u8; 3], radius: f32) {
        ui.painter()
            .rect_filled(self.rect(r), radius * self.scale(), color(col));
    }
    fn boxed(&self, ui: &Ui, r: [f32; 4], fill: [u8; 3], border: [u8; 3], radius: f32, width: f32) {
        ui.painter().rect(
            self.rect(r),
            radius * self.scale(),
            color(fill),
            Stroke::new(width * self.scale(), color(border)),
            StrokeKind::Inside,
        );
    }
    fn panel(&self, ui: &Ui, r: [f32; 4]) {
        self.boxed(ui, r, self.theme.panel, self.theme.border, 7., 1.);
    }
    fn line(&self, ui: &Ui, a: [f32; 2], b: [f32; 2], col: [u8; 3]) {
        ui.painter().line_segment(
            [self.point(a[0], a[1]), self.point(b[0], b[1])],
            Stroke::new(self.scale(), color(col)),
        );
    }
    fn circle(&self, ui: &Ui, x: f32, y: f32, r: f32, col: [u8; 3]) {
        ui.painter()
            .circle_filled(self.point(x, y), r * self.scale(), color(col));
    }
    fn text(
        &self,
        ui: &Ui,
        x: f32,
        y: f32,
        text: &str,
        size: f32,
        bold: bool,
        col: [u8; 3],
        align: Align2,
    ) {
        ui.painter().text(
            self.point(x, y),
            align,
            text,
            FontId::new(
                size * self.scale(),
                if bold {
                    FontFamily::Name("semibold".into())
                } else {
                    FontFamily::Proportional
                },
            ),
            color(col),
        );
    }
    fn image(&self, ui: &Ui, tex: &TextureHandle, r: [f32; 4], tint: [u8; 3]) {
        ui.painter().image(
            tex.id(),
            self.rect(r),
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            color(tint),
        );
    }
    fn control_text(
        &self,
        ui: &Ui,
        x: f32,
        y: f32,
        text: &str,
        bold: bool,
        col: [u8; 3],
        align: Align2,
    ) {
        let font = if bold {
            buttons::primary_font(self.scale())
        } else {
            FontId::proportional(COMPACT_BUTTON_FONT_SIZE * self.scale())
        };
        let galley = buttons::centered_galley(
            ui,
            ui.painter()
                .layout_no_wrap(text.to_owned(), font, color(col)),
        );
        let origin = align.anchor_size(self.point(x, y), galley.size()).min;
        ui.painter().galley(origin, galley, color(col));
    }
    fn button(
        &self,
        ui: &mut Ui,
        id: impl std::hash::Hash,
        r: [f32; 4],
        label: &str,
        icon: Option<Icon>,
        primary: bool,
    ) -> bool {
        self.styled_button(ui, id, r, label, icon, primary, false)
    }
    fn update_button(&self, ui: &mut Ui, id: impl std::hash::Hash, r: [f32; 4]) -> bool {
        self.styled_button(ui, id, r, "Update", None, true, true)
    }
    fn styled_button(
        &self,
        ui: &mut Ui,
        id: impl std::hash::Hash,
        r: [f32; 4],
        label: &str,
        icon: Option<Icon>,
        primary: bool,
        update: bool,
    ) -> bool {
        let mut r = button_bounds(r);
        r[1] += (r[3] - COMPACT_BUTTON_HEIGHT) / 2.;
        r[3] = COMPACT_BUTTON_HEIGHT;
        let font = if primary {
            buttons::primary_font(self.scale())
        } else {
            FontId::new(
                COMPACT_BUTTON_FONT_SIZE * self.scale(),
                FontFamily::Proportional,
            )
        };
        let text_width = ui
            .painter()
            .layout_no_wrap(label.to_owned(), font, color(self.theme.text))
            .size()
            .x
            / self.sx;
        let needed = text_width
            + if icon.is_some() {
                18. * BUTTON_SCALE + 5.
            } else {
                0.
            }
            + 2. * buttons::HORIZONTAL_PADDING;
        let extra = (needed - r[2]).max(0.);
        r[0] -= extra / 2.;
        r[2] += extra;
        // Paint and align contents against the same physical pixel rectangle.
        // Otherwise frame tessellation can round a fractional edge separately
        // from the text and leave identical buttons with different baselines.
        let painted_rect = self
            .rect(r)
            .round_to_pixels(ui.painter().pixels_per_point());
        r = [
            (painted_rect.left() - self.origin.x) / self.sx,
            (painted_rect.top() - self.origin.y) / self.sy,
            painted_rect.width() / self.sx,
            painted_rect.height() / self.sy,
        ];
        let response = ui.interact(self.rect(r), Id::new(id), Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
        let update_colors = buttons::update_colors(
            &self.theme,
            response.hovered() || response.is_pointer_button_down_on(),
        );
        let bg = if update {
            update_colors.0
        } else if primary {
            if response.hovered() || response.is_pointer_button_down_on() {
                self.theme.accent_hover
            } else {
                self.theme.accent
            }
        } else if response.hovered() || response.is_pointer_button_down_on() {
            SIDEBAR_HIGHLIGHT
        } else if label == "Launch" {
            self.theme.button
        } else {
            self.theme.card
        };
        self.boxed(
            ui,
            r,
            bg,
            if primary {
                self.theme.accent
            } else {
                self.theme.border
            },
            BUTTON_RADIUS,
            if primary || response.hovered() || response.is_pointer_button_down_on() {
                0.
            } else {
                1.
            },
        );
        let fg = if update {
            update_colors.1
        } else if primary {
            self.theme.on_accent
        } else if response.hovered() || response.is_pointer_button_down_on() {
            SIDEBAR_ON_HIGHLIGHT
        } else {
            self.theme.text
        };
        if let Some(icon) = icon {
            let font = if primary {
                buttons::primary_font(self.scale())
            } else {
                FontId::new(
                    COMPACT_BUTTON_FONT_SIZE * self.scale(),
                    FontFamily::Proportional,
                )
            };
            let text_width = ui
                .painter()
                .layout_no_wrap(label.to_owned(), font, color(fg))
                .size()
                .x
                / self.sx;
            let icon_size = 18. * BUTTON_SCALE;
            let gap = 5.;
            let left = r[0] + (r[2] - text_width - icon_size - gap) / 2.;
            self.icon(
                ui,
                icon,
                left,
                r[1] + r[3] / 2. - icon_size / 2.,
                icon_size,
                fg,
            );
            self.control_text(
                ui,
                left + icon_size + gap,
                r[1] + r[3] / 2.,
                label,
                primary,
                fg,
                Align2::LEFT_CENTER,
            );
        } else {
            self.control_text(
                ui,
                r[0] + r[2] / 2.,
                r[1] + r[3] / 2.,
                label,
                primary,
                fg,
                Align2::CENTER_CENTER,
            );
        }
        response.on_hover_cursor(CursorIcon::PointingHand).clicked()
    }
    fn icon_button(
        &self,
        ui: &mut Ui,
        id: impl std::hash::Hash,
        r: [f32; 4],
        icon: Icon,
        primary: bool,
    ) -> bool {
        let mut r = button_bounds(r);
        r[0] += (r[2] - COMPACT_BUTTON_HEIGHT) / 2.;
        r[1] += (r[3] - COMPACT_BUTTON_HEIGHT) / 2.;
        r[2] = COMPACT_BUTTON_HEIGHT;
        r[3] = COMPACT_BUTTON_HEIGHT;
        let res = ui.interact(self.rect(r), Id::new(id), Sense::click());
        self.boxed(
            ui,
            r,
            if primary {
                if res.hovered() || res.is_pointer_button_down_on() {
                    self.theme.accent_hover
                } else {
                    self.theme.accent
                }
            } else if res.hovered() || res.is_pointer_button_down_on() {
                SIDEBAR_HIGHLIGHT
            } else {
                self.theme.card
            },
            self.theme.border,
            BUTTON_RADIUS,
            if res.hovered() || res.is_pointer_button_down_on() {
                0.
            } else {
                1.
            },
        );
        let icon_size = if matches!(icon, Icon::Grid | Icon::List) {
            VIEW_MODE_ICON_SIZE
        } else {
            18. * BUTTON_SCALE
        };
        self.icon(
            ui,
            icon,
            r[0] + (r[2] - icon_size) / 2.,
            r[1] + (r[3] - icon_size) / 2.,
            icon_size,
            if primary {
                self.theme.on_accent
            } else if res.hovered() || res.is_pointer_button_down_on() {
                SIDEBAR_ON_HIGHLIGHT
            } else {
                self.theme.muted
            },
        );
        res.on_hover_cursor(CursorIcon::PointingHand).clicked()
    }
    fn pill(&self, ui: &mut Ui, id: &str, r: [f32; 4], text: &str, active: bool) -> bool {
        let mut r = button_bounds(r);
        r[1] += (r[3] - COMPACT_BUTTON_HEIGHT) / 2.;
        r[3] = COMPACT_BUTTON_HEIGHT;
        let res = ui.interact(self.rect(r), Id::new(id), Sense::click());
        self.boxed(
            ui,
            r,
            if active {
                if res.hovered() || res.is_pointer_button_down_on() {
                    self.theme.accent_hover
                } else {
                    self.theme.accent
                }
            } else {
                self.theme.card
            },
            if active {
                self.theme.accent_hover
            } else {
                self.theme.border
            },
            18. * BUTTON_SCALE,
            1.,
        );
        self.text(
            ui,
            r[0] + r[2] / 2.,
            r[1] + r[3] / 2.,
            text,
            COMPACT_BUTTON_FONT_SIZE,
            true,
            if active {
                self.theme.on_accent
            } else {
                self.theme.muted
            },
            Align2::CENTER_CENTER,
        );
        res.clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
    }
    fn nav(&self, ui: &mut Ui, label: &str, icon: Icon, y: f32, active: bool) -> bool {
        let res = ui
            .interact(
                self.rect([10., y, 220., 48.]),
                Id::new(label),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        if active || res.hovered() {
            self.fill(ui, [10., y, 220., 48.], SIDEBAR_HIGHLIGHT, 6.);
        }
        self.icon(
            ui,
            icon,
            34.,
            y + 13.,
            24.,
            if active || res.hovered() {
                SIDEBAR_ON_HIGHLIGHT
            } else {
                self.theme.muted
            },
        );
        self.text(
            ui,
            77.,
            y + 24.,
            label,
            16.,
            false,
            if active || res.hovered() {
                SIDEBAR_ON_HIGHLIGHT
            } else {
                self.theme.muted
            },
            Align2::LEFT_CENTER,
        );
        res.clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
    }
    fn window_button(&self, ui: &mut Ui, x: f32, text: &str) -> bool {
        let res = ui
            .interact(
                self.rect([x - 15., 0., 30., 30.]),
                Id::new(text),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        if res.hovered() {
            self.fill(ui, [x - 15., 0., 30., 30.], SIDEBAR_HIGHLIGHT, 2.);
        }
        self.text(
            ui,
            x,
            15.,
            text,
            16.,
            false,
            if res.hovered() {
                SIDEBAR_ON_HIGHLIGHT
            } else {
                self.theme.subtle
            },
            Align2::CENTER_CENTER,
        );
        res.clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
    }
    fn icon(&self, ui: &Ui, icon: Icon, x: f32, y: f32, size: f32, col: [u8; 3]) {
        let p = ui.painter();
        let s = Stroke::new(1.7 * self.scale(), color(col));
        let point = |a: f32, b: f32| self.point(x + a * size / 24., y + b * size / 24.);
        let line = |a: [f32; 2], b: [f32; 2]| {
            p.line_segment([point(a[0], a[1]), point(b[0], b[1])], s);
        };
        match icon {
            Icon::User => {
                p.circle_stroke(point(12., 7.), size * self.scale() / 6., s);
                p.add(Shape::line(
                    vec![
                        point(4., 22.),
                        point(4., 19.),
                        point(5., 16.),
                        point(8., 14.),
                        point(16., 14.),
                        point(19., 16.),
                        point(20., 19.),
                        point(20., 22.),
                    ],
                    s,
                ));
            }
            Icon::Check => {
                p.circle_stroke(point(12., 12.), size * self.scale() * 0.46, s);
                line([6., 12.], [10., 16.]);
                line([10., 16.], [18., 8.]);
            }
            Icon::Back => {
                line([20., 12.], [4., 12.]);
                line([4., 12.], [10., 6.]);
                line([4., 12.], [10., 18.]);
            }
            Icon::Reset => {
                p.add(Shape::line(
                    (0..=24)
                        .map(|i| {
                            let a = -2.3 + i as f32 * 5.3 / 24.;
                            point(12. + 8. * a.cos(), 12. + 8. * a.sin())
                        })
                        .collect(),
                    s,
                ));
                line([3., 3.], [3., 10.]);
                line([3., 10.], [10., 10.]);
            }
            Icon::Layers => {
                for y in [7., 12., 17.] {
                    line([3., y], [12., y + 4.]);
                    line([12., y + 4.], [21., y]);
                    if y == 7. {
                        line([3., y], [12., y - 4.]);
                        line([12., y - 4.], [21., y]);
                    }
                }
            }
            Icon::Storage => {
                p.add(Shape::closed_line(
                    vec![
                        point(5., 4.),
                        point(19., 4.),
                        point(22., 19.),
                        point(2., 19.),
                    ],
                    s,
                ));
                line([3., 14.], [21., 14.]);
                for x in [7., 11., 17.] {
                    p.circle_filled(point(x, 17.), self.scale(), s.color);
                }
            }
            Icon::Database => {
                for y in [5., 12., 19.] {
                    p.add(Shape::ellipse_stroke(
                        point(12., y),
                        vec2(size * self.sx * 0.38, size * self.sy * 0.13),
                        s,
                    ));
                }
                line([3., 5.], [3., 19.]);
                line([21., 5.], [21., 19.]);
            }
            Icon::Speed => {
                p.add(Shape::line(
                    (0..=24)
                        .map(|i| {
                            let a = std::f32::consts::PI + i as f32 * std::f32::consts::PI / 24.;
                            point(12. + 10. * a.cos(), 16. + 10. * a.sin())
                        })
                        .collect(),
                    s,
                ));
                line([12., 16.], [17., 8.]);
                for (a, b) in [
                    ([4., 14.], [6., 14.]),
                    ([7., 8.], [8., 10.]),
                    ([12., 6.], [12., 8.]),
                    ([19., 14.], [21., 14.]),
                ] {
                    line(a, b);
                }
            }
            Icon::Folder => {
                p.add(Shape::closed_line(
                    vec![
                        point(2., 5.),
                        point(9., 5.),
                        point(12., 8.),
                        point(22., 8.),
                        point(22., 22.),
                        point(2., 22.),
                    ],
                    s,
                ));
            }
            Icon::Home => {
                p.add(Shape::convex_polygon(
                    vec![
                        point(1., 11.),
                        point(12., 1.),
                        point(23., 11.),
                        point(20., 11.),
                        point(20., 23.),
                        point(14., 23.),
                        point(14., 15.),
                        point(9., 15.),
                        point(9., 23.),
                        point(4., 23.),
                        point(4., 11.),
                    ],
                    color(col),
                    Stroke::NONE,
                ));
            }
            Icon::Cube => {
                for (a, b) in [
                    ([2., 6.], [12., 1.]),
                    ([12., 1.], [22., 6.]),
                    ([22., 6.], [22., 18.]),
                    ([22., 18.], [12., 23.]),
                    ([12., 23.], [2., 18.]),
                    ([2., 18.], [2., 6.]),
                    ([2., 6.], [12., 11.]),
                    ([12., 11.], [22., 6.]),
                    ([12., 11.], [12., 23.]),
                ] {
                    line(a, b);
                }
            }
            Icon::Chip => {
                p.rect_stroke(
                    Rect::from_min_max(point(5., 5.), point(19., 19.)),
                    1.,
                    s,
                    StrokeKind::Inside,
                );
                for k in [7., 12., 17.] {
                    line([k, 1.], [k, 4.]);
                    line([k, 20.], [k, 23.]);
                    line([1., k], [4., k]);
                    line([20., k], [23., k]);
                }
            }
            Icon::Plus => {
                line([12., 2.], [12., 22.]);
                line([2., 12.], [22., 12.]);
            }
            Icon::Download | Icon::Upload => {
                line([2., 17.], [2., 23.]);
                line([2., 23.], [22., 23.]);
                line([22., 23.], [22., 17.]);
                line([12., 2.], [12., 17.]);
                if matches!(icon, Icon::Download) {
                    line([5., 11.], [12., 18.]);
                    line([12., 18.], [19., 11.]);
                } else {
                    line([5., 9.], [12., 2.]);
                    line([12., 2.], [19., 9.]);
                }
            }
            Icon::Search => {
                p.circle_stroke(point(10., 10.), 7. * size / 24. * self.scale(), s);
                line([15., 15.], [23., 23.]);
            }
            Icon::Down => {
                line([6., 9.], [12., 15.]);
                line([12., 15.], [18., 9.]);
            }
            Icon::Dots => {
                for k in [4., 12., 20.] {
                    p.circle_filled(point(12., k), 1.7 * self.scale(), color(col));
                }
            }
            Icon::Grid => {
                for (a, b) in [(2., 2.), (14., 2.), (2., 14.), (14., 14.)] {
                    p.rect_filled(
                        Rect::from_min_max(point(a, b), point(a + 8., b + 8.)),
                        0.,
                        color(col),
                    );
                }
            }
            Icon::List => {
                for k in [5., 12., 19.] {
                    line([2., k], [22., k]);
                }
            }
            Icon::Book => {
                p.add(Shape::closed_line(
                    vec![
                        point(12., 4.),
                        point(5., 2.),
                        point(2., 3.),
                        point(2., 21.),
                        point(6., 20.),
                        point(12., 22.),
                        point(18., 20.),
                        point(22., 21.),
                        point(22., 3.),
                        point(19., 2.),
                    ],
                    s,
                ));
                line([12., 4.], [12., 22.]);
            }
            Icon::Gear => {
                p.circle_stroke(point(12., 12.), 7. * size / 24. * self.scale(), s);
                p.circle_stroke(point(12., 12.), 3. * size / 24. * self.scale(), s);
                for k in 0..8 {
                    let a = k as f32 * std::f32::consts::TAU / 8.;
                    line(
                        [12. + a.cos() * 8., 12. + a.sin() * 8.],
                        [12. + a.cos() * 11., 12. + a.sin() * 11.],
                    );
                }
            }
            Icon::Pin => {
                line([8., 2.], [17., 2.]);
                line([9., 2.], [8., 10.]);
                line([16., 2.], [17., 10.]);
                line([8., 10.], [5., 15.]);
                line([5., 15.], [20., 15.]);
                line([20., 15.], [17., 10.]);
                line([12., 15.], [12., 23.]);
            }
        }
    }
}

// Preferred card widths determine column count; each row shares the remaining width.
const PAGE_HEADER_DIVIDER_Y: f32 = 102.;
const FILTER_DIVIDER_GAP: f32 = 6.;
const FILTER_ROW_HEIGHT: f32 = 34.;
const FILTER_ROW_GAP: f32 = 12.;
struct PageHeaderLayout {
    actions_y: f32,
    title_y: f32,
    subtitle_y: f32,
    divider_y: f32,
}
impl PageHeaderLayout {
    fn new() -> Self {
        Self {
            actions_y: 42.,
            title_y: 40.,
            subtitle_y: 73.,
            divider_y: PAGE_HEADER_DIVIDER_Y,
        }
    }
    fn filters_y(&self) -> f32 {
        self.divider_y + FILTER_DIVIDER_GAP
    }
}
const PROJECT_CARD_WIDTH: f32 = 200.;
const LIBRARY_CARD_WIDTH: f32 = 240.8;
const PROJECT_CARD_GAP: f32 = 14.;
const LIBRARY_THUMBNAIL_ASPECT: f32 = (LIBRARY_CARD_WIDTH - 2.) / 124.;
fn card_grid_metrics(available: f32, preferred: f32, gap: f32) -> (usize, f32) {
    let available = available.max(1.);
    let columns = card_columns(available, preferred, gap);
    let width = (available - gap * (columns - 1) as f32) / columns as f32;
    (columns, width)
}
fn card_columns(available: f32, width: f32, gap: f32) -> usize {
    ((available + gap) / (width + gap)).floor().max(1.) as usize
}

#[cfg(test)]
mod responsive_tests {
    use super::{
        LIBRARY_CARD_WIDTH, PROJECT_CARD_GAP, PROJECT_CARD_WIDTH, card_columns, card_grid_metrics,
    };
    #[test]
    fn grids_fill_available_width_without_changing_column_targets() {
        for available in [1., 689., 1009., 1517., 1537., 1609., 2249.] {
            for preferred in [PROJECT_CARD_WIDTH, LIBRARY_CARD_WIDTH] {
                let (columns, width) = card_grid_metrics(available, preferred, PROJECT_CARD_GAP);
                assert_eq!(
                    columns,
                    card_columns(available, preferred, PROJECT_CARD_GAP)
                );
                assert!(width > 0.);
                let right = columns as f32 * width + (columns - 1) as f32 * PROJECT_CARD_GAP;
                assert!((right - available).abs() < 0.001);
            }
        }
        assert_eq!(
            card_grid_metrics(1517., PROJECT_CARD_WIDTH, PROJECT_CARD_GAP).0,
            7
        );
        assert_eq!(
            card_grid_metrics(1517., LIBRARY_CARD_WIDTH, PROJECT_CARD_GAP).0,
            6
        );
    }
    #[test]
    fn cards_reflow_at_exact_width_boundaries() {
        assert_eq!(card_columns(301., 301., 12.), 1);
        assert_eq!(card_columns(613., 301., 12.), 1);
        assert_eq!(card_columns(614., 301., 12.), 2);
        assert_eq!(card_columns(1280., 301., 12.), 4);
        assert_eq!(card_columns(1906., 301., 12.), 6);
        assert_eq!(card_columns(1262., 240.8, 14.), 5);
        for viewport in [1000., 1200., 1586., 1920., 2560.] {
            let available = viewport - 306.;
            let columns = card_columns(available, 301., 12.);
            assert!(columns as f32 * 313. - 12. <= available);
            assert!((columns + 1) as f32 * 313. - 12. > available);
        }
    }
}

// Borderless windows need explicit native resize hit targets.
fn resize_window_edges(ctx: &Context) {
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }
    let rect = ctx.content_rect();
    let Some(point) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    if let Some((direction, cursor)) = resize_edge(rect, point) {
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}
pub(crate) fn resize_edge(
    rect: Rect,
    point: Pos2,
) -> Option<(egui::viewport::ResizeDirection, CursorIcon)> {
    use egui::viewport::ResizeDirection::*;
    if !rect.contains(point) {
        return None;
    }
    let left = point.x < rect.left() + 6.;
    let right = point.x > rect.right() - 6.;
    let top = point.y < rect.top() + 6.;
    let bottom = point.y > rect.bottom() - 6.;
    let near_left = point.x < rect.left() + 16.;
    let near_right = point.x > rect.right() - 16.;
    let near_top = point.y < rect.top() + 16.;
    let near_bottom = point.y > rect.bottom() - 16.;
    Some(if (left && near_top) || (top && near_left) {
        (NorthWest, CursorIcon::ResizeNwSe)
    } else if (right && near_top) || (top && near_right) {
        (NorthEast, CursorIcon::ResizeNeSw)
    } else if (left && near_bottom) || (bottom && near_left) {
        (SouthWest, CursorIcon::ResizeNeSw)
    } else if (right && near_bottom) || (bottom && near_right) {
        (SouthEast, CursorIcon::ResizeNwSe)
    } else if left {
        (West, CursorIcon::ResizeHorizontal)
    } else if right {
        (East, CursorIcon::ResizeHorizontal)
    } else if top {
        (North, CursorIcon::ResizeVertical)
    } else if bottom {
        (South, CursorIcon::ResizeVertical)
    } else {
        return None;
    })
}

#[cfg(test)]
mod resize_tests {
    use super::*;
    use egui::viewport::ResizeDirection::*;
    #[test]
    fn all_edges_and_corners_have_native_resize_targets() {
        let r = Rect::from_min_size(Pos2::ZERO, vec2(1000., 680.));
        for (x, y, d) in [
            (2., 2., NorthWest),
            (998., 2., NorthEast),
            (2., 678., SouthWest),
            (998., 678., SouthEast),
            (2., 340., West),
            (998., 340., East),
            (500., 2., North),
            (500., 678., South),
        ] {
            assert_eq!(resize_edge(r, pos2(x, y)).map(|v| v.0), Some(d));
        }
        assert!(resize_edge(r, pos2(500., 340.)).is_none());
        assert!(resize_edge(r, pos2(979., 17.)).is_none()); // close button
    }
}

// A moving segment communicates activity without inventing a percentage.
fn indeterminate_progress(ui: &mut Ui, width: f32, height: f32, accent: [u8; 3], track: [u8; 3]) {
    let (rect, _) = ui.allocate_exact_size(vec2(width.max(1.), height), Sense::hover());
    ui.painter().rect_filled(rect, height / 2., color(track));
    let fraction = ((ui.input(|i| i.time) * 2.).sin() as f32 + 1.) / 2.;
    let segment_width = rect.width() * 0.24;
    let segment = Rect::from_min_size(
        pos2(
            rect.left() + (rect.width() - segment_width) * fraction,
            rect.top(),
        ),
        vec2(segment_width, height),
    );
    ui.painter()
        .rect_filled(segment, height / 2., color(accent));
    ui.ctx().request_repaint_after(Duration::from_millis(16));
}
