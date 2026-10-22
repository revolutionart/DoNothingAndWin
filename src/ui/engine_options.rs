use super::*;
use engine_workspace::{installer::Plan, maintenance};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct Options {
    path: String,
    stamp: String,
    receiver: Option<Receiver<Result<Plan, String>>>,
    plan: Option<Plan>,
    original: Vec<String>,
    fresh_install: bool,
    sizes: HashMap<String, u64>,
    disk: u64,
    download: u64,
    error: String,
}
fn label(tag: &str) -> &str {
    match tag {
        "templates" => "Templates and Feature Packs",
        "engine_source" => "Engine Source",
        "metahuman_content" => "MetaHuman Creator Core Data",
        "editor_symbols" => "Editor symbols for debugging",
        "platform_Android" => "Android",
        "platform_IOS" => "iOS",
        "platform_Linux" => "Linux",
        "platform_TVOS" => "tvOS",
        "platform_WinARM64" => "Windows ARM64",
        "platform_VOS" => "visionOS",
        _ => tag,
    }
}
fn size(bytes: u64) -> String {
    if bytes == 0 {
        "0 B".into()
    } else if bytes < 1_000_000_000 {
        format!("{:.2} MB", bytes as f64 / 1_000_000.)
    } else {
        format!("{:.2} GB", bytes as f64 / 1_000_000_000.)
    }
}
impl Options {
    pub(super) fn from_install_plan(plan: Plan) -> Self {
        let mut options = Self {
            fresh_install: true,
            ..Self::default()
        };
        for f in &plan.manifest.files {
            if f.tags.is_empty() {
                *options.sizes.entry(String::new()).or_default() += f.size();
            }
            for tag in &f.tags {
                *options.sizes.entry(tag.clone()).or_default() += f.size();
            }
        }
        options.plan = Some(plan);
        options.totals();
        options
    }
    fn totals(&mut self) {
        let Some(plan) = &self.plan else {
            return;
        };
        let wanted = plan.files();
        self.disk = wanted.iter().map(|f| f.size()).sum();
        let chunks: HashSet<_> = wanted
            .iter()
            .filter(|f| {
                self.fresh_install
                    || (!f.tags.is_empty() && !f.tags.iter().any(|t| self.original.contains(t)))
            })
            .flat_map(|f| f.parts.iter().map(|p| &p.guid))
            .collect();
        self.download = chunks
            .iter()
            .filter_map(|id| plan.manifest.chunks.get(*id))
            .map(|c| c.size)
            .sum();
    }
}
fn option_row(
    ui: &mut Ui,
    theme: &Theme,
    title: &str,
    amount: &str,
    checked: &mut bool,
    enabled: bool,
    subtitle: Option<&str>,
) -> bool {
    let (r, response) = ui.allocate_exact_size(
        vec2(
            ui.available_width(),
            if subtitle.is_some() { 48. } else { 40. },
        ),
        Sense::click(),
    );
    let changed = enabled
        && response
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked();
    if changed {
        *checked = !*checked;
    }
    let box_rect = Rect::from_min_size(pos2(r.left() + 2., r.top() + 9.), vec2(22., 22.));
    let fill = if !enabled {
        theme.button
    } else if *checked {
        if response.hovered() {
            theme.accent_hover
        } else {
            theme.accent
        }
    } else {
        theme.card
    };
    ui.painter().rect_filled(box_rect, 3., color(fill));
    if !*checked {
        ui.painter().rect_stroke(
            box_rect,
            3.,
            Stroke::new(1., color(theme.muted)),
            StrokeKind::Inside,
        );
    }
    if *checked {
        ui.painter().line_segment(
            [box_rect.min + vec2(5., 11.), box_rect.min + vec2(9., 15.)],
            Stroke::new(
                2.,
                color(if enabled {
                    theme.on_accent
                } else {
                    theme.muted
                }),
            ),
        );
        ui.painter().line_segment(
            [box_rect.min + vec2(9., 15.), box_rect.min + vec2(18., 6.)],
            Stroke::new(
                2.,
                color(if enabled {
                    theme.on_accent
                } else {
                    theme.muted
                }),
            ),
        );
    }
    let text_rect = Rect::from_min_max(
        pos2(r.left() + 38., r.top()),
        pos2(r.right() - 88., r.bottom()),
    );
    ui.painter().with_clip_rect(text_rect).text(
        pos2(r.left() + 38., r.top() + 20.),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(14.),
        color(if enabled { theme.text } else { theme.muted }),
    );
    ui.painter().text(
        pos2(r.right(), r.top() + 20.),
        Align2::RIGHT_CENTER,
        amount,
        FontId::proportional(13.),
        color(theme.muted),
    );
    if let Some(subtitle) = subtitle {
        ui.painter().text(
            pos2(r.left() + 38., r.top() + 40.),
            Align2::LEFT_CENTER,
            subtitle,
            FontId::proportional(11.),
            color(theme.muted),
        );
    }
    response.on_hover_text(title);
    changed
}
fn component_panel(ui: &mut Ui, theme: &Theme, options: &mut Options, platform: bool) -> bool {
    let mut changed = false;
    Frame::new()
        .fill(color(theme.card))
        .stroke(Stroke::new(1., color(theme.border)))
        .corner_radius(6.)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width((ui.available_width() - 2.).max(180.));
            ui.spacing_mut().item_spacing.y = 6.;
            ui.label(
                RichText::new(if platform {
                    "Target platforms"
                } else {
                    "Engine components"
                })
                .size(20.)
                .strong()
                .color(ui.visuals().text_color()),
            );
            ui.separator();
            if !platform {
                option_row(
                    ui,
                    theme,
                    "Core Components",
                    &size(*options.sizes.get("").unwrap_or(&0)),
                    &mut true,
                    false,
                    Some("Required"),
                );
                ui.separator();
            }
            let known = if platform {
                vec![
                    "platform_Android",
                    "platform_IOS",
                    "platform_Linux",
                    "platform_TVOS",
                    "platform_WinARM64",
                    "platform_VOS",
                ]
            } else {
                vec![
                    "templates",
                    "engine_source",
                    "metahuman_content",
                    "editor_symbols",
                ]
            };
            let Some(plan) = &mut options.plan else {
                return;
            };
            let mut tags: Vec<String> = known.iter().map(|t| (*t).into()).collect();
            for tag in &plan.tags {
                if tag.starts_with("platform_") == platform && !tags.contains(tag) {
                    tags.push(tag.clone());
                }
            }
            for tag in tags {
                let available = plan.tags.contains(&tag);
                if tag == "platform_VOS" && !available {
                    continue;
                }
                let mut checked = plan.selected.contains(&tag);
                let amount = if available {
                    size(*options.sizes.get(&tag).unwrap_or(&0))
                } else {
                    "Unavailable".into()
                };
                if option_row(
                    ui,
                    theme,
                    label(&tag),
                    &amount,
                    &mut checked,
                    available,
                    if tag == "platform_TVOS" {
                        Some("Requires iOS")
                    } else {
                        None
                    },
                ) {
                    changed = true;
                    if checked {
                        plan.selected.push(tag.clone());
                    } else {
                        plan.selected.retain(|t| t != &tag);
                    }
                    if tag == "platform_TVOS"
                        && checked
                        && !plan.selected.contains(&"platform_IOS".into())
                    {
                        plan.selected.push("platform_IOS".into());
                    }
                    if tag == "platform_IOS" && !checked {
                        plan.selected.retain(|t| t != "platform_TVOS");
                    }
                }
                ui.separator();
            }
        });
    changed
}
impl Workspace {
    pub(super) fn install_component_dialog(&mut self, ctx: &Context) {
        let Some(options) = self.transfer.component_options.as_mut() else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        dialog::show_with_icon(
            ctx,
            &self.theme.clone(),
            "install-component-options",
            "Installation options",
            "Choose engine components and target platforms.",
            &mut open,
            None,
            |ui| {
                let mut changed = false;
                if ui.available_width() >= 780. {
                    ui.columns(2, |cols| {
                        changed |= component_panel(&mut cols[0], &self.theme, options, false);
                        changed |= component_panel(&mut cols[1], &self.theme, options, true);
                    });
                } else {
                    changed |= component_panel(ui, &self.theme, options, false);
                    changed |= component_panel(ui, &self.theme, options, true);
                }
                if changed {
                    options.totals();
                }
                ui.separator();
                ui.label(format!(
                    "Download size: {}    Required storage: {}",
                    size(options.download),
                    size(options.disk)
                ));
                apply = dialog::footer(ui, &self.theme, "Apply", true, false);
            },
        );
        if apply {
            if let (Some(plan), Some(options)) = (
                self.transfer.operation.plan_mut(),
                &self.transfer.component_options,
            ) {
                plan.selected = options.plan.as_ref().unwrap().selected.clone();
            }
        }
        if apply || !open {
            self.transfer.component_options = None;
        }
    }
    pub(super) fn engine_options_dialog(&mut self, ctx: &Context) {
        let Some(engine) = self.engine_options.clone() else {
            return;
        };
        let stamp = format!(
            "{}:{:?}:{:?}",
            engine.version,
            std::fs::metadata(Path::new(&engine.path).join(".engine-workspace-manifest.json.gz"))
                .and_then(|m| m.modified())
                .ok(),
            std::fs::metadata(Path::new(&engine.path).join("Engine/Build/Build.version"))
                .and_then(|m| m.modified())
                .ok()
        );
        if self.options.path != engine.path
            || (self.options.receiver.is_none() && self.options.stamp != stamp)
        {
            self.options = Options {
                path: engine.path.clone(),
                stamp: stamp.clone(),
                ..Default::default()
            };
            if self.demo {
                self.options.error =
                    "Component options require an installed engine's Epic manifest.".into();
            } else {
                let (tx, rx) = mpsc::channel();
                self.options.receiver = Some(rx);
                let engine = engine.clone();
                let session = storage::load_session();
                let snapshot = self.snapshot.clone();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let result = maintenance::prepare(&engine, session.as_ref(), &snapshot).map(
                        |mut plan| {
                            plan.tags = plan
                                .manifest
                                .files
                                .iter()
                                .flat_map(|f| f.tags.clone())
                                .collect();
                            plan.tags.sort();
                            plan.tags.dedup();
                            plan
                        },
                    );
                    let _ = tx.send(result);
                    ctx.request_repaint();
                });
            }
        }
        if let Some(result) = self
            .options
            .receiver
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.options.receiver = None;
            match result {
                Ok(plan) => {
                    self.options.stamp = format!(
                        "{}:{:?}:{:?}",
                        engine.version,
                        std::fs::metadata(
                            Path::new(&engine.path).join(".engine-workspace-manifest.json.gz")
                        )
                        .and_then(|m| m.modified())
                        .ok(),
                        std::fs::metadata(
                            Path::new(&engine.path).join("Engine/Build/Build.version")
                        )
                        .and_then(|m| m.modified())
                        .ok()
                    );
                    self.options.original = plan.selected.clone();
                    for f in &plan.manifest.files {
                        if f.tags.is_empty() {
                            *self.options.sizes.entry(String::new()).or_default() += f.size();
                        }
                        for tag in &f.tags {
                            *self.options.sizes.entry(tag.clone()).or_default() += f.size();
                        }
                    }
                    self.options.plan = Some(plan);
                    self.options.totals();
                }
                Err(e) => self.options.error = e,
            }
        }
        let mut open = true;
        let mut apply = false;
        let busy = self.transfer.busy() || self.plugin_work.mutating;
        dialog::show_with_icon(
            ctx,
            &self.theme.clone(),
            "engine-options",
            &format!("Engine options - UE {}", engine.version),
            "Choose components and target platforms for this installation.",
            &mut open,
            None,
            |ui| {
                if self.options.receiver.is_some() {
                    ui.horizontal(|ui| {
                        ui.add(loading::spinner(&self.theme));
                        ui.label("Loading installed engine components...");
                    });
                }
                if !self.options.error.is_empty() {
                    ui.label(RichText::new(&self.options.error).color(color(self.theme.warning)));
                }
                if self.options.plan.is_some() {
                    let mut changed = false;
                    if ui.available_width() >= 780. {
                        ui.columns(2, |cols| {
                            changed |= component_panel(
                                &mut cols[0],
                                &self.theme,
                                &mut self.options,
                                false,
                            );
                            changed |=
                                component_panel(&mut cols[1], &self.theme, &mut self.options, true);
                        });
                    } else {
                        changed |= component_panel(ui, &self.theme, &mut self.options, false);
                        changed |= component_panel(ui, &self.theme, &mut self.options, true);
                    }
                    if changed {
                        self.options.totals();
                    }
                    Frame::new()
                        .fill(color(self.theme.card))
                        .corner_radius(6.)
                        .inner_margin(16)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.columns(2, |cols| {
                                cols[0].label("Download size");
                                cols[0].label(
                                    RichText::new(size(self.options.download))
                                        .strong()
                                        .color(cols[0].visuals().text_color()),
                                );
                                cols[1].label("Required storage");
                                cols[1].label(
                                    RichText::new(size(self.options.disk))
                                        .strong()
                                        .color(cols[1].visuals().text_color()),
                                );
                            });
                        });
                    ui.label(RichText::new("Download size covers added components. Damaged existing files may require additional repair downloads.").size(12.).color(color(self.theme.muted)));
                }
                let dirty = self.options.plan.as_ref().is_some_and(|p| {
                    let a: HashSet<_> = p.selected.iter().collect();
                    let b: HashSet<_> = self.options.original.iter().collect();
                    a != b
                });
                apply = dialog::footer(ui, &self.theme, "Apply changes", dirty && !busy, false);
            },
        );
        if !open {
            self.engine_options = None;
            if let Some(plan) = &mut self.options.plan {
                plan.selected = self.options.original.clone();
            }
            self.options.totals();
        }
        if apply {
            let mut original = self.options.plan.take().unwrap();
            let selected = original.selected.clone();
            original.selected = self.options.original.clone();
            self.transfer = transfers::Transfers {
                state: "Applying engine component changes".into(),
                ..Default::default()
            };
            let Some(worker) = self.transfer.operation.maintain(
                engine_workspace::engine_operation::Kind::Components,
                engine.clone(),
            ) else {
                return;
            };
            let (tx, control, progress) = (worker.events, worker.control, worker.progress);
            self.engine_options = None;
            self.options = Options::default();
            self.page = Page::Downloads;
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result =
                    maintenance::change_components(&original, selected, &control, progress);
                let _ = tx.send(transfers::Event::Maintained(engine, false, result));
                ctx.request_repaint();
            });
        }
    }
}
