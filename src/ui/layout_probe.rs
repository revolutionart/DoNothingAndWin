use super::*;
use engine_workspace::discovery;

// Opt-in render regression probe. Uses the real native window and renderer;
// never starts transfers or writes workspace preferences.
pub(super) struct LayoutProbe {
    directory: std::path::PathBuf,
    step: usize,
    phase: u8,
    deadline: Instant,
    activity: bool,
    engine_activity: bool,
    engine_update: bool,
    plugin_backups: bool,
    plugin_design: bool,
    project_finder: bool,
    library_organization: bool,
    plugin_install: bool,
    library_queue: bool,
    empty_projects: bool,
    page: Option<Page>,
}
const CASES: &[(f32, f32, bool)] = &[
    (1000., 680., false),
    (1200., 800., false),
    (1400., 900., false),
    (1586., 992., false),
    (1920., 1080., false),
    (1000., 680., true),
    (1200., 800., true),
    (1400., 900., true),
    (1586., 992., true),
    (1920., 1080., true),
];
impl LayoutProbe {
    pub(super) fn from_args() -> Option<Self> {
        let directory = std::env::args().find_map(|a| {
            a.strip_prefix("--layout-probe=")
                .map(std::path::PathBuf::from)
        })?;
        std::fs::create_dir_all(&directory).expect("create layout probe directory");
        Some(Self {
            directory,
            step: 0,
            phase: 0,
            deadline: Instant::now(),
            activity: std::env::args().any(|a| a == "--plugin-activity-preview"),
            engine_activity: std::env::args().any(|a| a == "--engine-activity-preview"),
            engine_update: std::env::args().any(|a| a == "--engine-update-preview"),
            plugin_backups: std::env::args().any(|a| a == "--plugin-backups-preview"),
            plugin_design: std::env::args().any(|a| a == "--plugin-design-preview"),
            project_finder: std::env::args().any(|a| a == "--project-finder-preview"),
            plugin_install: std::env::args().any(|a| {
                a == "--plugin-install-preview"
                    || a == "--queued-install-preview"
                    || a == "--plugin-download-preview"
            }),
            library_queue: std::env::args()
                .any(|a| a == "--library-queue-preview" || a == "--queued-install-preview"),
            library_organization: std::env::args().any(|a| a == "--library-organization-preview"),
            empty_projects: std::env::args().any(|a| a == "--empty-projects-preview"),
            page: std::env::args().find_map(|a| match a.as_str() {
                "--settings-preview" => Some(Page::Settings),
                "--library-organization-preview"
                | "--plugin-install-preview"
                | "--plugin-download-preview"
                | "--queued-install-preview" => Some(Page::Library),
                "--page-preview=downloads" | "--library-queue-preview" => Some(Page::Downloads),
                "--page-preview=library" => Some(Page::Library),
                "--page-preview=engines" => Some(Page::Engines),
                "--page-preview=projects" => Some(Page::Projects),
                "--page-preview=workspace" => Some(Page::Workspace),
                _ => None,
            }),
        })
    }
}
impl Workspace {
    pub(super) fn run_layout_probe(&mut self, ctx: &Context) {
        let Some(mut probe) = self.layout_probe.take() else {
            return;
        };
        if probe.step >= if probe.page.is_some() { 5 } else { CASES.len() } {
            if probe.library_queue
                || std::env::args().any(|a| a == "--plugin-download-activity-preview")
            {
                self.finish_library_preview();
            }
            self.plugin_work.mutating = false;
            self.transfer.operation = Default::default();
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        let (width, height, plugins) = CASES[probe.step];
        if probe.phase == 0 {
            if std::env::args().any(|arg| arg == "--scan-results-preview") {
                self.scan.summary = "Scan complete: 21 projects; 2155 folders searched; 1 inaccessible folder; 1 linked folder skipped.".into();
                self.scan.skipped_folders = vec![
                    discovery::SkippedFolder {
                        path: "D:/Unreal Projects/Shared Studio/Restricted project archive".into(),
                        reason: "Could not read folder: Access is denied. (os error 5)".into(),
                        linked: false,
                    },
                    discovery::SkippedFolder {
                        path: "D:/Unreal Projects/Linked projects".into(),
                        reason: "Folder link or reparse point was not followed.".into(),
                        linked: true,
                    },
                ];
            }
            if probe.activity || probe.engine_activity || probe.engine_update {
                self.plugin_work.mutating = false;
                self.plugin_work.progress = None;
                self.transfer.operation = Default::default();
            }
            ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(30., 30.)));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(width, height)));
            self.plugin_engine = None;
            self.project_scroll = 0.;
            if plugins && !self.engines.is_empty() {
                self.show_plugins((self.engines.len() - 1).min(1));
                if probe.plugin_design {
                    // Render-only comparison states. No package is fetched or installed.
                    let rows = self
                        .plugins
                        .iter()
                        .enumerate()
                        .map(|(index, plugin)| engine_workspace::plugins::Checked {
                            plugin: plugin.clone(),
                            available_version: Some(if index == 0 {
                                "2.85".into()
                            } else {
                                plugin.version.clone()
                            }),
                            update: (index == 0).then(|| engine_workspace::plugins::Update {
                                version: "2.85".into(),
                                number: 285,
                                manifest: Default::default(),
                                bases: vec![],
                                prefix: Default::default(),
                            }),
                            status: if index == 1 {
                                "Managed by its publisher".into()
                            } else {
                                "Up to date".into()
                            },
                        })
                        .collect();
                    if let Some(i) = self.plugin_engine {
                        self.library.catalogue.checked(
                            self.engines[i].path.clone(),
                            Instant::now(),
                            rows,
                        );
                    }
                    self.plugin_work.status = "Design preview".into();
                    if std::env::args().any(|a| a == "--plugin-check-preview") {
                        self.plugin_work.preview_check(self.plugins.len());
                    }
                }
            }
            if probe.activity {
                self.plugin_work.preview_activity();
                if !plugins {
                    self.page = Page::Downloads;
                }
            }
            if probe.engine_activity && !plugins {
                self.page = Page::Downloads;
                self.transfer.activity = Default::default();
                let now = Instant::now();
                let mut totals = [0; 3];
                for i in 0..120 {
                    totals[0] += 12_500_000 + (i % 9) * 400_000;
                    totals[1] += if i % 13 == 0 { 6_000_000 } else { 0 };
                    totals[2] += 22_000_000 + (i % 5) * 800_000;
                    self.transfer.activity.sample_totals(
                        "engine",
                        totals,
                        now - Duration::from_millis((120 - i) * 500),
                    );
                }
                self.transfer.operation.preview(
                    engine_workspace::engine_operation::Kind::Install,
                    engine_workspace::installer::Progress {
                        phase: "Downloading and verifying chunks".into(),
                        completed: 760_000_000,
                        total: 13_910_000_000,
                        package_bytes: 13_910_000_000,
                        network: totals[0],
                        read: totals[1],
                        written: totals[2],
                        files_total: 50_000,
                        ..Default::default()
                    },
                );
            }
            if probe.engine_update {
                self.page = Page::Downloads;
                self.transfer.state = "Updating Unreal Engine 5.8.3 to 5.8.4".into();
                let mut p = engine_workspace::installer::Progress {
                    network: 5_540_000_000,
                    ..Default::default()
                };
                match probe.step {
                    0 | 1 => {
                        p.phase = "Final verification of updated engine files".into();
                        p.completed = 18_000_000_000;
                        p.total = 30_020_000_000;
                        p.files = 125_030;
                        p.files_total = 209_441;
                        p.detail = "Engine/Binaries/Win64/UnrealEditor-Renderer.dll".into();
                    }
                    2 => {
                        p.phase = "Preserving installed plugins and custom files".into();
                        p.detail = "210030 entries checked • Engine/Plugins/Marketplace/PCGEx/Binaries/Win64/UnrealEditor-PCGExtendedToolkit.dll".into();
                    }
                    3 => p.phase = "Registering updated engine with Epic Games Launcher".into(),
                    _ => p.phase = "Removing previous engine backup".into(),
                }
                self.transfer
                    .operation
                    .preview(engine_workspace::engine_operation::Kind::Update, p);
            }
            if probe.library_organization {
                self.preview_library_organization(probe.step);
            }
            if probe.plugin_install {
                self.preview_plugin_install();
                if std::env::args().any(|a| a == "--plugin-download-preview") {
                    self.preview_plugin_download();
                }
            }
            if probe.plugin_backups {
                self.preview_plugin_backups(probe.step);
            }
            if probe.library_queue {
                self.preview_library_queue();
            }
            if let Some(page) = probe.page {
                self.page = page;
            }
            if std::env::args().any(|a| a == "--plugin-download-activity-preview") {
                self.preview_plugin_download_activity(probe.step > 0);
            }
            if std::env::args().any(|a| a == "--project-backup-preview") {
                self.preview_project_backup(probe.step % 2 == 1);
            }
            if probe.empty_projects {
                self.projects.clear();
            }
            if probe.project_finder {
                self.project_finder.preview();
            }
            probe.phase = 1;
            probe.deadline = Instant::now() + Duration::from_millis(700);
        } else if probe.phase == 1 && Instant::now() >= probe.deadline {
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
            probe.phase = 2;
        } else if probe.phase == 2 {
            let screenshot = ctx.input(|i| {
                i.events.iter().find_map(|e| {
                    if let Event::Screenshot { image, .. } = e {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(image) = screenshot {
                let name = format!(
                    "{}-{}x{}",
                    match probe.page {
                        Some(Page::Settings) => "settings",
                        Some(Page::Downloads) => "downloads",
                        Some(Page::Library) => "library",
                        Some(Page::Engines) => "engines",
                        Some(Page::Projects) => "projects",
                        _ =>
                            if plugins {
                                "plugins"
                            } else {
                                "workspace"
                            },
                    },
                    width,
                    height
                );
                let pixels: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                image::save_buffer(
                    probe.directory.join(format!("{name}.png")),
                    &pixels,
                    image.width() as u32,
                    image.height() as u32,
                    image::ColorType::Rgba8,
                )
                .expect("save probe screenshot");
                std::fs::write(
                    probe.directory.join(format!("{name}.txt")),
                    format!(
                        "Requested: {width}x{height}\nActual logical client: {:?}\n",
                        ctx.content_rect().size()
                    ),
                )
                .expect("save probe dimensions");
                probe.step += 1;
                probe.phase = 0;
            }
        }
        if probe.plugin_install {
            self.library_install_dialog(ctx);
        }
        self.layout_probe = Some(probe);
        ctx.request_repaint_after(Duration::from_millis(40));
    }
}
