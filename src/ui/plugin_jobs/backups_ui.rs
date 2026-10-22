use super::*;
use engine_workspace::plugins::backups::{self, Backup};
fn quantity(n: u64, unit: &str) -> String {
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

#[derive(Clone)]
enum Action {
    Refresh,
    Keep(String, bool),
    Delete(String),
    Restore(String),
}
struct Outcome {
    action: Action,
    result: Result<String, String>,
    entries: Result<Vec<Backup>, String>,
}
pub(super) struct Manager {
    engine: Installation,
    entries: Vec<Backup>,
    rx: Option<Receiver<Outcome>>,
    status: String,
    confirm: Option<Action>,
    preview: bool,
}
impl Workspace {
    pub(super) fn open_plugin_backups(&mut self, engine: Installation, ctx: Context) {
        if self.plugin_work.busy() || self.transfer.busy() || self.library_work_pending() {
            return;
        }
        self.plugin_work.backups = Some(Manager {
            engine,
            entries: vec![],
            rx: None,
            status: String::new(),
            confirm: None,
            preview: false,
        });
        self.start_backup_action(Action::Refresh, ctx);
    }
    fn start_backup_action(&mut self, action: Action, ctx: Context) {
        if self.plugin_work.busy()
            || self.transfer.busy()
            || self.library_work_pending()
            || self.launcher_updates.blocks_work()
        {
            return;
        }
        let Some(manager) = self.plugin_work.backups.as_mut() else {
            return;
        };
        if manager.rx.is_some() || manager.preview {
            return;
        }
        let engine = manager.engine.clone();
        let days = self.prefs.plugin_backup_days;
        let (tx, rx) = mpsc::channel();
        manager.rx = Some(rx);
        manager.confirm = None;
        manager.status = match action {
            Action::Refresh => "Loading plugin backups…",
            Action::Keep(..) => "Saving backup retention…",
            Action::Delete(..) => "Deleting plugin backup…",
            Action::Restore(..) => "Preparing plugin rollback…",
        }
        .into();
        self.plugin_work.mutating = true;
        self.plugin_work.status = manager.status.clone();
        self.plugin_work.operation = "Plugin rollback".into();
        let progress = Arc::new(Mutex::new(PluginProgress {
            title: format!("Plugin backups • UE {}", engine.version),
            ..Default::default()
        }));
        self.plugin_work.progress = matches!(action, Action::Restore(..)).then(|| progress.clone());
        std::thread::spawn(move || {
            let result = match &action {
                Action::Refresh => backups::refresh(&engine).map(|_| String::new()),
                Action::Keep(id, kept) => backups::set_kept(&engine, id, *kept, days)
                    .map(|_| "Backup retention saved.".into()),
                Action::Delete(id) => backups::delete(&engine, id)
                    .map(|_| "Deleted backup. The installed plugin is unchanged.".into()),
                Action::Restore(id) => backups::restore(&engine, id, days, |state| {
                    progress.lock().unwrap().state = state;
                    ctx.request_repaint();
                }),
            };
            let entries = backups::list(&engine);
            let _ = tx.send(Outcome {
                action,
                result,
                entries,
            });
            ctx.request_repaint();
        });
    }
    pub(super) fn poll_plugin_backups(&mut self, ctx: &Context) {
        let Some(manager) = self.plugin_work.backups.as_mut() else {
            return;
        };
        let Some(rx) = &manager.rx else { return };
        ctx.request_repaint_after(Duration::from_millis(150));
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Outcome {
                action: Action::Refresh,
                result: Err("Plugin backup worker stopped unexpectedly. Saved backups have not been discarded.".into()),
                entries: Err("Refresh the backup list before trying again.".into()),
            },
        };
        manager.rx = None;
        let restored = matches!(outcome.action, Action::Restore(..));
        let engine = manager.engine.clone();
        manager.status = outcome
            .result
            .clone()
            .unwrap_or_else(|e| format!("Backup operation failed: {e}"));
        match outcome.entries {
            Ok(entries) => manager.entries = entries,
            Err(error) => manager.status.push_str(&format!(" {error}")),
        }
        self.plugin_work.mutating = false;
        self.plugin_work.progress = None;
        self.plugin_work.status = manager.status.clone();
        if restored {
            self.record_activity(
                "Plugin rollback".into(),
                engine.version.clone(),
                &outcome.result,
            );
            self.plugins = plugins::installed(Path::new(&engine.path));
            self.plugin_counts
                .insert(engine.path.clone(), self.plugins.len());
            self.workspace_cache.invalidate();
            // An old comparison must not label a restored version up to date.
            self.library
                .catalogue
                .checked(engine.path, Instant::now(), vec![]);
        }
    }
    pub(super) fn plugin_backups_dialog(&mut self, ctx: &Context) {
        let Some(manager) = &self.plugin_work.backups else {
            return;
        };
        let engine = manager.engine.clone();
        let entries = manager.entries.clone();
        let status = manager.status.clone();
        let confirm = manager.confirm.clone();
        let busy = manager.rx.is_some()
            || self.plugin_work.busy()
            || self.transfer.busy()
            || self.library_work_pending();
        let mut open = true;
        let mut close = false;
        let mut action = None;
        let mut request = None;
        let mut cancel = false;
        let theme = self.theme.clone();
        dialog::show(
            ctx,
            &theme,
            "plugin-backups",
            "Plugin backups",
            &format!(
                "Plugin versions saved for Unreal Engine {}.",
                engine.version
            ),
            &mut open,
            |ui| {
                ui.label("Restore a plugin without changing the engine version. Backups are created before updates and restores. Keep a backup to prevent automatic deletion.");
                if ui
                    .add_enabled(!busy, buttons::secondary_button("Refresh backups"))
                    .clicked()
                {
                    action = Some(Action::Refresh);
                }
                if entries.is_empty() && !busy {
                    ui.label("No plugin backups yet. Your next plugin update will save the current version here.");
                }
                for backup in &entries {
                    ui.push_id(&backup.id, |ui| {
                        Frame::new()
                            .fill(color(theme.card))
                            .stroke(Stroke::new(1., color(theme.border)))
                            .corner_radius(6.)
                            .inner_margin(14.)
                            .show(ui, |ui| {
                                ui.set_width((ui.available_width() - 2.).max(200.));
                                ui.label(
                                    RichText::new(format!("{} • {}", backup.name, backup.version))
                                        .strong()
                                        .color(color(theme.text)),
                                );
                                let age =
                                    engine_workspace::epic::now().saturating_sub(backup.created);
                                let saved = if age < 60 {
                                    "just now".into()
                                } else if age < 3600 {
                                    format!("{} ago", quantity(age / 60, "minute"))
                                } else if age < 86400 {
                                    format!("{} ago", quantity(age / 3600, "hour"))
                                } else {
                                    format!("{} ago", quantity(age / 86400, "day"))
                                };
                                let retention = if backup.kept {
                                    "Kept until manually deleted".into()
                                } else if let Some(expires) = backup.expires {
                                    let seconds =
                                        expires.saturating_sub(engine_workspace::epic::now());
                                    if seconds == 0 {
                                        "Expired — removed on next cleanup".into()
                                    } else {
                                        format!(
                                            "Expires in {}",
                                            quantity(seconds.div_ceil(86400), "day")
                                        )
                                    }
                                } else {
                                    "Kept until manually deleted".into()
                                };
                                ui.label(
                                    RichText::new(format!(
                                        "Saved {saved} • {:.1} MB • {retention}",
                                        backup.bytes as f64 / 1_000_000.
                                    ))
                                    .color(color(theme.muted)),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    let mut kept = backup.kept;
                                    if ui
                                        .add_enabled(
                                            !busy,
                                            buttons::checkbox(&mut kept, "Keep", &theme),
                                        )
                                        .changed()
                                    {
                                        action = Some(Action::Keep(backup.id.clone(), kept));
                                    }
                                    if ui
                                        .add_enabled(!busy, primary_button("Restore", &theme))
                                        .clicked()
                                    {
                                        request = Some(Action::Restore(backup.id.clone()));
                                    }
                                    if ui
                                        .add_enabled(
                                            !busy,
                                            buttons::secondary_button("Delete backup"),
                                        )
                                        .clicked()
                                    {
                                        request = Some(Action::Delete(backup.id.clone()));
                                    }
                                });
                            });
                    });
                }
                if let Some(confirm) = &confirm {
                    let id = match confirm {
                        Action::Restore(id) | Action::Delete(id) => id,
                        _ => unreachable!(),
                    };
                    if let Some(backup) = entries.iter().find(|b| &b.id == id) {
                        ui.separator();
                        let restoring = matches!(confirm, Action::Restore(..));
                        ui.label(if restoring {
                            format!("Restore {} {} in UE {}? The current plugin will be saved as another backup. Close Unreal Editor first.",backup.name,backup.version,engine.version)
                        } else {
                            format!("Permanently delete the backup of {} {}? The installed plugin will stay unchanged.",backup.name,backup.version)
                        });
                        ui.horizontal(|ui| {
                            let mut danger = theme.clone();
                            danger.accent = theme.danger;
                            danger.accent_hover = theme.danger;
                            danger.on_accent = [255, 255, 255];
                            if ui
                                .add_enabled(
                                    !busy,
                                    if restoring {
                                        primary_button("Restore backup", &theme)
                                    } else {
                                        primary_button("Delete permanently", &danger)
                                    },
                                )
                                .clicked()
                            {
                                action = Some(confirm.clone());
                            }
                            if ui
                                .add_enabled(!busy, buttons::secondary_button("Cancel"))
                                .clicked()
                            {
                                cancel = true;
                            }
                        });
                    }
                }
                if busy {
                    ui.add(loading::spinner(&theme));
                    if self.plugin_work.progress.is_some() {
                        self.plugin_work
                            .show_progress(ui, &theme, &self.textures, false);
                    }
                }
                if !status.is_empty() {
                    ui.label(&status);
                }
                ui.separator();
                if ui
                    .add_enabled(!busy, buttons::secondary_button("Back to plugins"))
                    .clicked()
                {
                    close = true;
                }
            },
        );
        // Keep the manager alive to consume worker completion even if close is clicked.
        if (!open || close) && !busy {
            let preview = self.plugin_work.backups.as_ref().is_some_and(|m| m.preview);
            self.plugin_work.backups = None;
            if !preview {
                self.start_plugin_check(ctx.clone(), true);
            }
        } else {
            if let Some(manager) = self.plugin_work.backups.as_mut() {
                if cancel {
                    manager.confirm = None;
                }
                if request.is_some() {
                    manager.confirm = request;
                }
            }
            if let Some(action) = action {
                self.start_backup_action(action, ctx.clone());
            }
        }
    }
    pub(in crate::ui) fn preview_plugin_backups(&mut self, step: usize) {
        let now = engine_workspace::epic::now();
        let entries = [("PCGEx","0.79.1",1_250_000_000u64,false),("Ascent Combat Framework Ultimate","4.1.0",738_000_000,true)]
            .into_iter().enumerate().map(|(i,(name,version,bytes,kept))| {
                serde_json::from_value(serde_json::json!({
                    "id":format!("abc-{i}"),"name":name,"version":version,"created":now-86400,
                    "expires":now+6*86400,"kept":kept,"bytes":bytes,"schema":1,
                    "engine":"Z:/render-preview/UE_5.8","branch":"5.8","relative":"Engine/Plugins/Marketplace/Preview",
                    "descriptor":"Preview.uplugin","number":1,"complete":true,"files":[],
                    "registration":null,"package_receipt":null
                })).unwrap()
            }).collect();
        self.plugin_work.backups = Some(Manager {
            engine: Installation {
                path: "Z:/render-preview/UE_5.8".into(),
                version: "5.8.3".into(),
                association: "5.8".into(),
            },
            entries,
            rx: None,
            status: String::new(),
            confirm: match step {
                2 => Some(Action::Restore("abc-0".into())),
                3 => Some(Action::Delete("abc-1".into())),
                _ => None,
            },
            preview: true,
        });
    }
}
