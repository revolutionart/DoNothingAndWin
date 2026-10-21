use super::*;
use engine_workspace::{
    download_policy,
    storage::{Activity, DownloadSettings},
};
#[derive(Default)]
pub(super) struct Downloads {
    pub settings: bool,
    recent: bool,
    search: String,
    history: Vec<Activity>,
    save_error: Option<String>,
    saved_at: Option<Instant>,
}
impl Downloads {
    pub(super) fn show_overview(&mut self) {
        self.recent = false;
    }
    pub fn load() -> Self {
        Self {
            history: storage::read_json(&storage::data_dir().join("download-history.json"))
                .unwrap_or_default(),
            ..Default::default()
        }
    }
}
pub(super) fn draw_icon(ui: &Ui, theme: &Theme, icon: Icon, rect: Rect, tint: [u8; 3]) {
    let canvas = Canvas {
        origin: rect.min,
        width: rect.width(),
        height: rect.height(),
        sx: 1.,
        sy: 1.,
        theme: theme.clone(),
    };
    canvas.icon(ui, icon, 0., 0., rect.width(), tint);
}
fn icon_button(ui: &mut Ui, theme: &Theme, label: &str, icon: Icon) -> bool {
    let text = ui.painter().layout_no_wrap(
        label.into(),
        FontId::proportional(COMPACT_BUTTON_FONT_SIZE),
        color(theme.text),
    );
    let (rect, response) = ui.allocate_exact_size(
        vec2(text.size().x + 60., COMPACT_BUTTON_HEIGHT),
        Sense::click(),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    let hover = response.hovered() || response.is_pointer_button_down_on();
    ui.painter().rect(
        rect,
        BUTTON_RADIUS,
        color(if hover { SIDEBAR_HIGHLIGHT } else { theme.card }),
        if hover {
            Stroke::NONE
        } else {
            Stroke::new(1., color(theme.border))
        },
        StrokeKind::Inside,
    );
    draw_icon(
        ui,
        theme,
        icon,
        Rect::from_min_size(
            rect.min + vec2(14., (COMPACT_BUTTON_HEIGHT - 18.) / 2.),
            vec2(18., 18.),
        ),
        if hover {
            SIDEBAR_ON_HIGHLIGHT
        } else {
            theme.text
        },
    );
    ui.painter().galley_with_override_text_color(
        rect.min + vec2(40., (COMPACT_BUTTON_HEIGHT - text.size().y) / 2.),
        text,
        color(if hover {
            SIDEBAR_ON_HIGHLIGHT
        } else {
            theme.text
        }),
    );
    response.on_hover_cursor(CursorIcon::PointingHand).clicked()
}
fn panel<R>(
    ui: &mut Ui,
    theme: &Theme,
    title: &str,
    subtitle: Option<&str>,
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    Frame::new()
        .fill(color(theme.card))
        .stroke(Stroke::new(1., color(theme.border)))
        .corner_radius(6.)
        .inner_margin(20)
        .show(ui, |ui| {
            ui.set_width((ui.available_width() - 2.).max(100.));
            if title != "Latest activity" {
                ui.horizontal(|ui| {
                    let icon = match title {
                        "Installation preferences" => Icon::Gear,
                        "Download speed" => Icon::Speed,
                        "Install location" => Icon::Folder,
                        "Fab asset cache" => Icon::Database,
                        "Download queue" => Icon::Layers,
                        "Storage" => Icon::Storage,
                        "Latest activity" => Icon::List,
                        _ => Icon::Search,
                    };
                    let (rect, _) = ui.allocate_exact_size(vec2(30., 30.), Sense::hover());
                    draw_icon(ui, theme, icon, rect, theme.text);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(title)
                                .size(22.)
                                .strong()
                                .color(ui.visuals().text_color()),
                        );
                        if let Some(s) = subtitle {
                            ui.label(RichText::new(s).size(14.).color(color(theme.muted)));
                        }
                    });
                });
                ui.add_space(10.);
                ui.separator();
                ui.add_space(10.);
            }
            body(ui)
        })
        .inner
}
pub(super) fn toggle(
    ui: &mut Ui,
    theme: &Theme,
    value: &mut bool,
    title: &str,
    description: &str,
) -> bool {
    let mut changed = false;
    ui.horizontal_top(|ui| {
        let (r, response) = ui.allocate_exact_size(vec2(46., 26.), Sense::click());
        if response
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            *value = !*value;
            changed = true;
        }
        response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, *value, title));
        ui.painter().rect_filled(
            r,
            13.,
            color(if *value { theme.accent } else { theme.button }),
        );
        ui.painter().circle_filled(
            pos2(
                if *value {
                    r.right() - 13.
                } else {
                    r.left() + 13.
                },
                r.center().y,
            ),
            9.,
            Color32::WHITE,
        );
        ui.add_space(8.);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(title)
                    .strong()
                    .color(ui.visuals().text_color())
                    .size(16.),
            );
            ui.label(
                RichText::new(description)
                    .color(color(theme.muted))
                    .size(14.),
            );
        });
    });
    ui.add_space(14.);
    changed
}
impl Workspace {
    pub(super) fn record_activity(
        &mut self,
        item: String,
        engine: String,
        result: &Result<String, String>,
    ) {
        if self.demo {
            return;
        }
        let success = result.is_ok();
        let message = match result {
            Ok(s) | Err(s) => s.clone(),
        };
        self.downloads.history.insert(
            0,
            Activity {
                item,
                engine,
                completed: epic::now(),
                success,
                message: message.clone(),
                seen: false,
            },
        );
        self.downloads.history.truncate(500);
        if let Err(e) = storage::write_json(
            &storage::data_dir().join("download-history.json"),
            &self.downloads.history,
        ) {
            self.notify(format!("Could not save download history: {e}"));
        }
        if success && self.prefs.downloads.notify_complete {
            crate::window_frame::completion_notice(&message);
        }
    }
    pub(super) fn downloads_view(&mut self, ui: &mut Ui, c: &Canvas) {
        let theme = self.theme.clone();
        let header = PageHeaderLayout::new();
        if c.button(
            ui,
            "download-settings",
            button_slot([c.width - 246., header.actions_y, 220., 34.]),
            if self.downloads.settings {
                "Back to downloads"
            } else {
                "Download settings"
            },
            Some(if self.downloads.settings {
                Icon::Back
            } else {
                Icon::Gear
            }),
            false,
        ) {
            self.downloads.settings = !self.downloads.settings;
        }
        c.line(
            ui,
            [266., header.divider_y],
            [c.width - 26., header.divider_y],
            theme.border,
        );
        let top = header.divider_y + 15.;
        let rect = c.rect([266., top, c.width - 292., (c.height - top - 52.).max(200.)]);
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.spacing_mut().item_spacing = vec2(12., 10.);
            ui.style_mut().override_font_id = Some(FontId::proportional(16.));
            AppScrollArea::vertical(&theme)
                .id_salt("downloads-content")
                .show(ui, |ui| {
                    ui.set_width(ui.available_width().max(250.));
                    if self.downloads.settings {
                        self.download_settings(ui, &theme);
                    } else {
                        self.download_overview(ui, &theme);
                    }
                });
        });
    }
    fn download_overview(&mut self, ui: &mut Ui, t: &Theme) {
        let recent_count = self.downloads.history.iter().filter(|a| a.success).count();
        ui.horizontal(|ui| {
            for (recent, label) in [
                (false, "Overview".to_owned()),
                (true, format!("Recently updated ({recent_count})")),
            ] {
                let selected = self.downloads.recent == recent;
                let response = buttons::tab_button(ui, t, selected, label, 116.);
                if response
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.downloads.recent = recent;
                }
            }
        });
        ui.add_space(6.);
        let active = self.transfer.busy() as usize
            + self.plugin_work.mutating as usize
            + self.library_transfer_status().is_some() as usize;
        if ui.available_width() > 900. {
            let main_width = ui.available_width() - 290.;
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(main_width, 0.),
                    Layout::top_down(Align::Min),
                    |ui| self.download_main(ui, t, active),
                );
                ui.vertical(|ui| {
                    ui.set_width(270.);
                    self.download_side(ui, t, active);
                });
            });
        } else {
            self.download_main(ui, t, active);
            self.download_side(ui, t, active);
        }
    }
    fn download_main(&mut self, ui: &mut Ui, t: &Theme, active: usize) {
        let waiting = !self.library.install_queue.waiting().is_empty();
        if !self.downloads.recent {
            if self.plugin_work.mutating && self.plugin_work.progress.is_some() {
                self.plugin_work.show_progress(ui, t, &self.textures, true);
            } else if self.transfer.operation.transferring()
                && self.transfer.operation.kind()
                    == engine_workspace::engine_operation::Kind::Install
            {
                let p = self.transfer.operation.progress();
                let title = self
                    .transfer
                    .release
                    .as_ref()
                    .map(|r| {
                        format!(
                            "Unreal Engine {}",
                            r.version
                                .as_ref()
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| r.app.clone())
                        )
                    })
                    .unwrap_or_else(|| "Unreal Engine".into());
                let card = super::plugin_activity::Card::engine(&title, &p);
                self.transfer
                    .activity
                    .show_card(ui, t, &card, &self.textures, true);
                if ui
                    .add(buttons::secondary_button("Stop transfer"))
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.transfer.operation.stop();
                }
            } else {
                Frame::new()
                    .fill(color(t.card))
                    .stroke(Stroke::new(1.5, color(t.accent)))
                    .corner_radius(6.)
                    .inner_margin(22)
                    .show(ui, |ui| {
                        ui.set_width((ui.available_width() - 2.).max(200.));
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(64., 64.), Sense::hover());
                            draw_icon(
                                ui,
                                t,
                                if active == 0 && !waiting {
                                    Icon::Check
                                } else {
                                    Icon::Download
                                },
                                r,
                                t.accent,
                            );
                            ui.add_space(8.);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(if active == 0 && waiting {
                                        "Waiting to install"
                                    } else if active == 0 {
                                        "You're all caught up"
                                    } else {
                                        "Installation activity"
                                    })
                                    .size(25.)
                                    .strong()
                                    .color(ui.visuals().text_color()),
                                );
                                if active == 0 {
                                    ui.label(if waiting { "Queued plugins will start when the current operation finishes." } else { "No downloads in progress." });
                                }
                            });
                        });
                        if self.transfer.busy() {
                            ui.label(&self.transfer.state);
                            let p = self.transfer.operation.progress();
                            if p.total == 0 {
                                ui.horizontal(|ui| {
                                    ui.add(egui::Spinner::new().color(if self.theme.dark {
                                        Color32::WHITE
                                    } else {
                                        color(self.theme.text)
                                    }));
                                    ui.label(&p.phase);
                                });
                                indeterminate_progress(
                                    ui,
                                    ui.available_width(),
                                    8.,
                                    t.accent,
                                    t.border,
                                );
                            } else {
                                ui.label(&p.phase);
                                ui.add(
                                    ProgressBar::new(
                                        (p.completed as f32 / p.total as f32).clamp(0., 1.),
                                    )
                                    .show_percentage()
                                    .desired_width(ui.available_width()),
                                );
                                ui.label(format!(
                                    "{:.2} / {:.2} GB • {:.2} GB downloaded",
                                    p.completed as f64 / 1e9,
                                    p.total as f64 / 1e9,
                                    p.network as f64 / 1e9
                                ));
                            }
                            if p.files_total > 0 {
                                ui.label(format!("{} / {} files processed", p.files, p.files_total));
                            }
                            if !p.detail.is_empty() {
                                ui.add(
                                    egui::Label::new(RichText::new(&p.detail).color(color(t.muted)))
                                        .truncate(),
                                )
                                .on_hover_text(&p.detail);
                            }
                            if self.transfer.operation.can_stop()
                                && ui
                                    .add(buttons::secondary_button("Stop transfer"))
                                    .clone()
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .clicked()
                            {
                                self.transfer.operation.stop();
                            }
                        }
                        if let Some(s) = self.library_transfer_status() {
                            ui.add(egui::Spinner::new().color(if self.theme.dark {
                                Color32::WHITE
                            } else {
                                color(self.theme.text)
                            }));
                            ui.label(s);
                            self.library.transfer_controls(ui);
                        }
                        if self.plugin_work.mutating {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().color(if self.theme.dark {
                                    Color32::WHITE
                                } else {
                                    color(self.theme.text)
                                }));
                                ui.label(&self.plugin_work.status);
                            });
                            self.plugin_work.show_progress(ui, t, &self.textures, true);
                        }
                    });
            }
            if self.transfer.operation.can_resume() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&self.transfer.state);
                    if ui.add(buttons::secondary_button("Resume engine installation"))
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.transfer.modal = true;
                    }
                    if ui.add(buttons::secondary_button("Dismiss"))
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text("Clear this resume entry. Downloaded files and chunks are kept; install again to the same destination to reuse them.")
                        .clicked()
                    {
                        self.transfer.dismiss_install();
                    }
                });
            }
            if let Some(path) = self.transfer.operation.installed().cloned() {
                ui.horizontal(|ui| {
                    if ui
                        .add(buttons::secondary_button("Open engine folder"))
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        if let Err(e) = platform::open_external(&path.to_string_lossy()) {
                            self.notify(e);
                        }
                    }
                    let prereq = path.join("Engine/Extras/Redist/en-us/UEPrereqSetup_x64.exe");
                    if prereq.is_file()
                        && ui
                            .add(buttons::secondary_button("Run prerequisites"))
                            .clone()
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked()
                    {
                        if let Err(e) = platform::open_external(&prereq.to_string_lossy()) {
                            self.notify(e);
                        }
                    }
                });
            }
            ui.add_space(4.);
        }
        if !self.scan.summary.is_empty() {
            panel(ui, t, "Project discovery", None, |ui| {
                ui.label(&self.scan.summary);
                if self.scan.receiver.is_some() {
                    ui.label(format!(
                        "{} folders searched",
                        self.scan
                            .progress
                            .load(std::sync::atomic::Ordering::Relaxed)
                    ));
                    if ui
                        .add(buttons::secondary_button("Stop scan"))
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.scan
                            .cancel
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                } else {
                    project_finder::skipped_folders(ui, t, &self.scan.skipped_folders);
                }
            });
        }
        panel(ui, t, "Latest activity", None, |ui| {
            let wide = ui.available_width() > 900.;
            if !wide {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(26., 26.), Sense::hover());
                    draw_icon(ui, t, Icon::List, r, t.text);
                    ui.label(
                        RichText::new("Latest activity")
                            .size(22.)
                            .strong()
                            .color(ui.visuals().text_color()),
                    );
                });
            }
            ui.horizontal(|ui| {
                if wide {
                    let (r, _) = ui.allocate_exact_size(vec2(26., 26.), Sense::hover());
                    draw_icon(ui, t, Icon::List, r, t.text);
                    ui.add_sized(
                        [200., 34.],
                        Label::new(
                            RichText::new("Latest activity")
                                .size(22.)
                                .strong()
                                .color(ui.visuals().text_color()),
                        ),
                    );
                }
                search_field(
                    ui,
                    &mut self.downloads.search,
                    "Search activity...",
                    (ui.available_width() - 300.).max(100.),
                    t,
                    1.,
                );
                if ui
                    .add_enabled(
                        !self.downloads.history.is_empty(),
                        buttons::secondary_button("Clear history"),
                    )
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    let empty: Vec<Activity> = vec![];
                    match storage::write_json(
                        &storage::data_dir().join("download-history.json"),
                        &empty,
                    ) {
                        Ok(()) => {
                            self.downloads.history.clear();
                            self.notify("Download history cleared.");
                        }
                        Err(e) => self.notify(format!("Could not clear history: {e}")),
                    }
                }
                if ui
                    .add(buttons::secondary_button("Mark all as seen"))
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    for a in &mut self.downloads.history {
                        a.seen = true;
                    }
                    if let Err(e) = storage::write_json(
                        &storage::data_dir().join("download-history.json"),
                        &self.downloads.history,
                    ) {
                        self.notify(e);
                    }
                }
            });
            ui.separator();
            // Reserve the actual action-button width and all four column gaps.
            // Otherwise history rows expand their frame beyond the activity panel.
            let action_width = 38.;
            let width =
                (ui.available_width() - action_width - 4. * ui.spacing().item_spacing.x).max(180.);
            ui.horizontal(|ui| {
                activity_cell(
                    ui,
                    [width * 0.38, 24.],
                    Label::new("Item").halign(Align::LEFT),
                );
                activity_cell(
                    ui,
                    [width * 0.12, 24.],
                    Label::new("Engine").halign(Align::LEFT),
                );
                activity_cell(
                    ui,
                    [width * 0.23, 24.],
                    Label::new("Completed").halign(Align::LEFT),
                );
                activity_cell(
                    ui,
                    [width * 0.27, 24.],
                    Label::new("Status").halign(Align::LEFT),
                );
            });
            let query = self.downloads.search.to_lowercase();
            let mut count = 0;
            for a in &self.downloads.history {
                if !format!("{} {} {}", a.item_label(), a.engine, a.message)
                    .to_lowercase()
                    .contains(&query)
                {
                    continue;
                }
                if self.downloads.recent && !a.success {
                    continue;
                }
                count += 1;
                ui.horizontal(|ui| {
                    let name = if a.seen {
                        a.item_label()
                    } else {
                        format!("• {}", a.item_label())
                    };
                    activity_cell(
                        ui,
                        [width * 0.38, 38.],
                        Label::new(
                            RichText::new(name)
                                .strong()
                                .color(ui.visuals().text_color()),
                        )
                        .truncate()
                        .halign(Align::LEFT),
                    )
                    .on_hover_text(&a.message);
                    activity_cell(
                        ui,
                        [width * 0.12, 38.],
                        Label::new(&a.engine).truncate().halign(Align::LEFT),
                    );
                    let elapsed = epic::now().saturating_sub(a.completed);
                    let age = if elapsed < 60 {
                        "Just now".into()
                    } else if elapsed < 3600 {
                        format!("{} min ago", elapsed / 60)
                    } else if elapsed < 86400 {
                        format!("{} hours ago", elapsed / 3600)
                    } else {
                        format!("{} days ago", elapsed / 86400)
                    };
                    activity_cell(ui, [width * 0.23, 38.], Label::new(age).halign(Align::LEFT));
                    activity_cell(
                        ui,
                        [width * 0.27, 38.],
                        Label::new(RichText::new(a.status_label()).color(color(if a.success {
                            t.success
                        } else if a.stopped() {
                            t.warning
                        } else {
                            [220, 65, 65]
                        })))
                        .truncate()
                        .halign(Align::LEFT),
                    );
                    let (rect, response) = ui.allocate_exact_size(
                        vec2(action_width, COMPACT_BUTTON_HEIGHT),
                        Sense::click(),
                    );
                    let response = response
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text("Activity details");
                    response.widget_info(|| {
                        WidgetInfo::labeled(WidgetType::Button, true, "Activity details")
                    });
                    let fg = if response.hovered() {
                        SIDEBAR_ON_HIGHLIGHT
                    } else {
                        t.muted
                    };
                    if response.hovered() {
                        ui.painter().rect_filled(rect, 5., color(SIDEBAR_HIGHLIGHT));
                    }
                    let center = rect.center();
                    let stroke = Stroke::new(1.5, color(fg));
                    ui.painter().circle_stroke(center, 9., stroke);
                    ui.painter()
                        .line_segment([center - vec2(0., 5.), center + vec2(0., 1.)], stroke);
                    ui.painter()
                        .circle_filled(center + vec2(0., 5.), 1., color(fg));
                    Popup::menu(&response).frame(menu_frame(t)).show(|ui| {
                        ui.label(&a.message);
                    });
                });
                ui.separator();
            }
            if count == 0 {
                ui.label(if !query.is_empty() {
                    "No activity matches your search."
                } else if self.downloads.recent {
                    "No successful operations yet. Stopped and failed operations appear in Overview."
                } else {
                    "No activity to show. Completed operations will appear here."
                });
            }
        });
    }
    fn download_side(&mut self, ui: &mut Ui, t: &Theme, active: usize) {
        panel(ui, t, "Download queue", None, |ui| {
            ui.label(
                RichText::new(format!(
                    "{active} active item{}",
                    if active == 1 { "" } else { "s" }
                ))
                .size(24.)
                .strong()
                .color(ui.visuals().text_color()),
            );
            let waiting: Vec<_> = self
                .library
                .install_queue
                .waiting()
                .iter()
                .cloned()
                .collect();
            ui.label(if !waiting.is_empty() {
                "Waiting plugins start automatically in the order below."
            } else if active == 0 {
                "Your queue is empty."
            } else {
                "Current installations are shown in the activity panel."
            });
            if !waiting.is_empty() {
                ui.separator();
                ui.label(
                    RichText::new(format!("{} waiting", waiting.len()))
                        .strong()
                        .color(color(t.text)),
                );
                let mut remove = None;
                AppScrollArea::vertical(t)
                    .id_salt("waiting-plugin-installs")
                    .max_height(260.)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (index, job) in waiting.iter().enumerate() {
                            ui.push_id(index, |ui| {
                                ui.label(RichText::new(&job.item.title).color(color(t.text)))
                                    .on_hover_text(&job.item.title);
                                ui.label(
                                    RichText::new(format!(
                                        "UE {}",
                                        job.engines
                                            .iter()
                                            .map(|e| e.version.as_str())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    ))
                                    .color(color(t.muted)),
                                );
                                if ui
                                    .add(buttons::secondary_button("Remove"))
                                    .on_hover_text("Remove this waiting installation")
                                    .clicked()
                                {
                                    remove = Some(index);
                                }
                                if index + 1 < waiting.len() {
                                    ui.separator();
                                }
                            });
                        }
                    });
                if let Some(index) = remove {
                    self.library.install_queue.remove(index);
                }
                if waiting.len() > 1
                    && ui
                        .add(buttons::secondary_button("Clear waiting list"))
                        .clicked()
                {
                    self.library.install_queue.clear_waiting();
                }
            }
        });
        panel(ui, t, "Storage", None, |ui| {
            let root = Path::new(&self.prefs.install_root);
            let existing = root.ancestors().find(|p| p.exists()).unwrap_or(root);
            if let (Ok(free), Ok(total)) =
                (fs2::available_space(existing), fs2::total_space(existing))
            {
                ui.label(
                    RichText::new(format!("{} GB available", free / 1_000_000_000))
                        .strong()
                        .color(ui.visuals().text_color())
                        .size(23.),
                );
                ui.scope(|ui| {
                    ui.spacing_mut().interact_size.y = 12.;
                    ui.add(ProgressBar::new(if total > 0 {
                        1. - free as f32 / total as f32
                    } else {
                        0.
                    }));
                });
                ui.label(
                    RichText::new(&self.prefs.install_root)
                        .size(13.)
                        .color(color(t.muted)),
                );
            } else {
                ui.label("Storage information unavailable");
            }
            if ui
                .link("Manage locations →")
                .clone()
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked()
            {
                self.downloads.settings = true;
            }
        });
    }
    fn download_settings(&mut self, ui: &mut Ui, t: &Theme) {
        let before = self.prefs.downloads.clone();
        let old_root = self.prefs.install_root.clone();
        let old_cache = self.prefs.fab_cache.clone();
        self.install_preferences(ui, t);
        if ui.available_width() > 900. {
            ui.columns(2, |cols| {
                self.speed_preferences(&mut cols[0], t);
                self.location_preferences(&mut cols[1], t);
            });
        } else {
            self.speed_preferences(ui, t);
            self.location_preferences(ui, t);
        }
        panel(
            ui,
            t,
            "Fab asset cache",
            Some("Reuse verified downloads for assets and plugins."),
            |ui| {
                ui.label("Cache directory");
                dialog::folder(ui, &mut self.prefs.fab_cache);
                ui.horizontal(|ui| {
                    if icon_button(ui, t, "Reset", Icon::Reset) {
                        self.prefs.fab_cache = storage::Preferences::default().fab_cache;
                    }
                    ui.label(
                        RichText::new(
                            "Changing the location leaves existing cached files in place.",
                        )
                        .size(13.)
                        .color(color(t.muted)),
                    );
                });
            },
        );
        if icon_button(ui, t, "Reset to default settings", Icon::Reset) {
            self.prefs.downloads = DownloadSettings::default();
            let defaults = storage::Preferences::default();
            self.prefs.install_root = defaults.install_root;
            self.prefs.fab_cache = defaults.fab_cache;
        }
        if before != self.prefs.downloads
            || old_root != self.prefs.install_root
            || old_cache != self.prefs.fab_cache
        {
            self.prefs.projects = self.projects.clone();
            self.prefs.launch_engine = self.launch_path.clone();
            self.downloads.save_error = storage::save_preferences(&self.prefs).err();
            if self.downloads.save_error.is_none() {
                self.downloads.saved_at = Some(Instant::now());
            }
            download_policy::configure(&self.prefs.downloads);
        }
        if let Some(e) = &self.downloads.save_error {
            ui.label(RichText::new(e).color(color(t.warning)));
        } else {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(20., 20.), Sense::hover());
                draw_icon(ui, t, Icon::Check, r, t.success);
                ui.label(
                    RichText::new(if self.downloads.saved_at.is_some() {
                        "All changes saved"
                    } else {
                        "Changes save automatically"
                    })
                    .color(color(t.success)),
                );
            });
        }
    }
    fn location_preferences(&mut self, ui: &mut Ui, t: &Theme) {
        panel(
            ui,
            t,
            "Install location",
            Some("Choose where new engine installations are stored."),
            |ui| {
                ui.label("Default install directory");
                dialog::folder(ui, &mut self.prefs.install_root);
                ui.label(RichText::new("Plugins install into the engine you select. Assets install into the project you select.").size(14.).color(color(t.muted)));
            },
        );
    }
    fn install_preferences(&mut self, ui: &mut Ui, t: &Theme) {
        panel(
            ui,
            t,
            "Installation preferences",
            Some("Manage when installations run."),
            |ui| {
                toggle(
                    ui,
                    t,
                    &mut self.prefs.downloads.notify_complete,
                    "Notify when installation completes",
                    "Show a Windows notification when an installation finishes.",
                );
                toggle(
                    ui,
                    t,
                    &mut self.prefs.downloads.allow_editors,
                    "Allow installs while editors are running",
                    "Allow other installations while editors are open. Files in an active engine remain protected.",
                );
                ui.label(
                    RichText::new("All engine, plugin and asset updates are started manually.")
                        .size(14.)
                        .color(color(t.muted)),
                );
            },
        );
    }
    fn speed_preferences(&mut self, ui: &mut Ui, t: &Theme) {
        panel(
            ui,
            t,
            "Download speed",
            Some("Control your installation bandwidth."),
            |ui| {
                toggle(
                    ui,
                    t,
                    &mut self.prefs.downloads.limit_speed,
                    "Limit download speed",
                    "Shared limit across engine, plugin and asset transfers.",
                );
                ui.add_enabled_ui(self.prefs.downloads.limit_speed, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            DragValue::new(&mut self.prefs.downloads.megabytes_per_second)
                                .range(0.1..=10000.)
                                .speed(0.5),
                        );
                        ui.label("MB/s");
                    });
                });
            },
        );
    }
    pub(super) fn desktop_preferences(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        ui.label(
            RichText::new("Preferences")
                .strong()
                .color(ui.visuals().text_color())
                .size(23.),
        );
        if crate::window_frame::tray_enabled() {
            ui.label("Minimize keeps the launcher in the taskbar.");
            if toggle(
                ui,
                &t,
                &mut self.prefs.close_to_tray,
                "Close to system tray",
                "When enabled, X keeps the launcher running in the tray. Turn off to quit when pressing X.",
            ) {
                self.persist();
            }
        } else {
            ui.label("Minimize keeps the portable launcher in the taskbar. Close (X) exits the application.");
        }
        ui.add_space(12.);
        self.hidden_project_preferences(ui);
        ui.add_space(12.);
        let old = (self.prefs.run_at_startup, self.prefs.startup_in_tray);
        let mut changed = toggle(
            ui,
            &t,
            &mut self.prefs.run_at_startup,
            "Start with Windows",
            "Start DoNothing&Win Launcher when you sign in to Windows.",
        );
        if crate::window_frame::tray_enabled() {
            changed |= toggle(
                ui,
                &t,
                &mut self.prefs.startup_in_tray,
                "Start in system tray",
                "When starting with Windows, keep the launcher in the tray without a taskbar button. Turn off to show the window.",
            );
        }
        if changed {
            match download_policy::set_startup(
                self.prefs.run_at_startup,
                self.prefs.startup_in_tray && crate::window_frame::tray_enabled(),
            ) {
                Ok(()) => self.persist(),
                Err(e) => {
                    (self.prefs.run_at_startup, self.prefs.startup_in_tray) = old;
                    self.notify(e);
                }
            }
        }
    }
}

// Keep the whole column allocation while laying its label out from the left edge.
fn activity_cell(ui: &mut Ui, size: [f32; 2], label: Label) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(size[0], size[1]), Sense::hover());
    let mut cell = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    cell.add(label)
}
