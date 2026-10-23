use super::*;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct Download {
    pub(super) only: bool,
    pub(super) branch: String,
    custom: bool,
    location: String,
    cache_root: String,
    pub(super) active: bool,
    pub(super) progress: fab::plugin_download::Progress,
    completed: Option<PathBuf>,
}

impl Library {
    pub(in crate::ui) fn show_plugin_download_progress(&self, ui: &mut Ui) {
        if !self.download.active {
            return;
        }
        let p = &self.download.progress;
        if p.total > 0 {
            ui.add(
                ProgressBar::new((p.prepared as f32 / p.total as f32).clamp(0., 1.))
                    .show_percentage(),
            );
            ui.label(format!(
                "{:.1} / {:.1} MB • {:.1} MB downloaded • {} / {} files verified",
                p.prepared as f64 / 1_048_576.,
                p.total as f64 / 1_048_576.,
                p.downloaded as f64 / 1_048_576.,
                p.files,
                p.total_files
            ));
        }
        if !p.file.is_empty() {
            ui.add(Label::new(&p.file).truncate())
                .on_hover_text(&p.file);
        }
    }
}

impl Workspace {
    pub(in crate::ui) fn show_downloaded_plugin(&mut self, ui: &mut Ui) {
        if let Some(path) = self.library.download.completed.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.label("Plugin download complete");
                if ui
                    .add(buttons::secondary_button("Open plugin folder"))
                    .clicked()
                {
                    if let Err(error) = platform::open_external(&path.to_string_lossy()) {
                        self.notify(error);
                    }
                }
            });
            ui.add(Label::new(dialog::display_path(&path.to_string_lossy())).truncate())
                .on_hover_text(path.to_string_lossy());
        }
    }

    pub(super) fn plugin_download_options(
        &mut self,
        ui: &mut Ui,
        item: &Item,
        waiting: bool,
    ) -> bool {
        let branches = item.supported_branches();
        ui.spacing_mut().item_spacing.y = 8.;
        let d = &mut self.library.download;
        if !branches.contains(&d.branch) {
            d.branch = branches.last().cloned().unwrap_or_default();
        }
        ui.horizontal(|ui| {
            ui.label("UE version");
            AppComboBox::new(&self.theme, "plugin-download-version")
                .selected_text(if d.branch.is_empty() {
                    "No Windows builds".into()
                } else {
                    format!("UE {}", d.branch)
                })
                .show_ui(ui, |ui| {
                    for branch in branches.iter().rev() {
                        buttons::selectable_value(
                            ui,
                            &mut d.branch,
                            branch.clone(),
                            format!("UE {branch}"),
                        );
                    }
                });
        });
        ui.label("Choose your project's UE version. No installed engine is required.");
        ui.horizontal(|ui| {
            ui.label("Save to");
            buttons::selectable_value(ui, &mut d.custom, false, "Plugin cache");
            buttons::selectable_value(ui, &mut d.custom, true, "Custom folder");
        });
        let location = if d.custom {
            dialog::folder(ui, &mut d.location);
            PathBuf::from(&d.location)
        } else {
            let roots = self.prefs.vault_cache_roots();
            if roots.len() > 1 {
                AppComboBox::new(&self.theme, "plugin-download-cache")
                    .bounded_width(360.)
                    .preview_open(std::env::args().any(|arg| arg == "--cache-picker-preview"))
                    .selected_text(if d.cache_root.is_empty() {
                        "Choose cache location".into()
                    } else {
                        dialog::display_path(&d.cache_root)
                    })
                    .show_ui(ui, |ui| {
                        for root in &roots {
                            buttons::selectable_value(
                                ui,
                                &mut d.cache_root,
                                root.clone(),
                                dialog::display_path(root),
                            );
                        }
                    });
            }
            self.prefs
                .vault_download_root(&d.cache_root)
                .map(|root| fab::plugin_download::cache_directory(&root))
                .unwrap_or_default()
        };
        if !d.custom {
            ui.add(Label::new(dialog::display_path(&location.to_string_lossy())).truncate())
                .on_hover_text(location.to_string_lossy());
        }
        ui.label("Saves an extracted plugin in its own build folder. Includes source code when supplied by the publisher. Source engine builds may need to rebuild the plugin.");
        if waiting {
            ui.label("Finish the current transfer or library sync before downloading.");
        }
        dialog::footer(
            ui,
            &self.theme,
            "Download",
            !d.branch.is_empty()
                && location.is_absolute()
                && !waiting
                && !self.demo
                && self.session.is_some()
                && !self.launcher_updates.blocks_work(),
            false,
        )
    }

    pub(super) fn start_plugin_download(&mut self, item: Item, ctx: &Context) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let d = &mut self.library.download;
        let branch = d.branch.clone();
        let location = if d.custom {
            PathBuf::from(&d.location)
        } else {
            let Some(root) = self.prefs.vault_download_root(&d.cache_root) else {
                return;
            };
            fab::plugin_download::cache_directory(&root)
        };
        let cache_root = if d.custom {
            Some(PathBuf::from(&self.prefs.fab_cache))
        } else {
            self.prefs
                .vault_download_root(&d.cache_root)
                .map(PathBuf::from)
        };
        d.active = true;
        d.completed = None;
        d.progress = Default::default();
        let control = engine_workspace::installer::Control::default();
        let (tx, rx) = mpsc::channel();
        self.library.rx = Some(rx);
        self.library.installing = true;
        self.library.install_control = Some(control.clone());
        self.library.install_item = format!("Plugin download: {}", item.title);
        self.library.install_engine = branch.clone();
        self.library.status = format!("{} — Preparing download…", self.library.install_item);
        self.library.selected = None;
        self.page = Page::Downloads;
        self.downloads.show_overview();
        let wait_for_library = self.prefs.wait_for_library_plugin_builds;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = engine_workspace::download_policy::with_cache_root(cache_root, || {
                fab::plugin_download::download(
                    &session,
                    &item,
                    &branch,
                    &location,
                    wait_for_library,
                    &control,
                    |progress| {
                        let _ = tx.send(Event::PluginDownloadProgress(progress));
                        crate::window_frame::progress_repaint(&ctx);
                    },
                )
            });
            let _ = tx.send(Event::PluginDownloaded(result));
            ctx.request_repaint();
        });
    }

    pub(in crate::ui) fn reset_plugin_download_cache(&mut self) {
        self.library.download.cache_root = if self.prefs.ask_vault_cache {
            String::new()
        } else {
            self.prefs.fab_cache.clone()
        };
    }

    pub(super) fn finish_plugin_download(&mut self, result: Result<PathBuf, String>) {
        let message = result
            .as_ref()
            .map(|path| {
                format!(
                    "Plugin downloaded: {}",
                    dialog::display_path(&path.to_string_lossy())
                )
            })
            .map_err(Clone::clone);
        self.record_activity(
            self.library.install_item.clone(),
            self.library.install_engine.clone(),
            &message,
        );
        self.library.download.completed = result.ok();
        self.library.download.active = false;
        self.library.installing = false;
        self.library.install_control = None;
        self.library.rx = None;
        self.library.status = message.unwrap_or_else(|e| {
            if e.starts_with("Stopped.") {
                e
            } else {
                format!("Plugin download failed: {e}")
            }
        });
        self.notify(self.library.status.clone());
    }

    pub(in crate::ui) fn preview_plugin_download(&mut self) {
        self.preview_plugin_install();
        self.engines.clear();
        self.library.download.only = true;
        self.library.download.custom = std::env::args().any(|a| a == "--custom-download-preview");
        self.library.download.location = "C:\\Plugin downloads".into();
    }

    pub(in crate::ui) fn preview_plugin_download_activity(&mut self, complete: bool) {
        self.demo = true;
        self.library.installing = !complete;
        self.library.download.active = !complete;
        self.library.install_control =
            (!complete).then(engine_workspace::installer::Control::default);
        self.library.status = "Plugin download: Assets Cleaner — Downloading plugin files…".into();
        self.library.download.progress = fab::plugin_download::Progress {
            phase: "Downloading plugin files…".into(),
            file: "Binaries/Win64/UnrealEditor-AssetsCleaner.dll".into(),
            prepared: 42_000_000,
            total: 100_000_000,
            downloaded: 15_000_000,
            files: 12,
            total_files: 30,
        };
        self.library.download.completed = complete.then(|| {
            PathBuf::from("C:/Plugin downloads/AssetsCleaner/UE_5.8-example/AssetsCleaner")
        });
    }
}
