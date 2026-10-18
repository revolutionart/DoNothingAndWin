use super::*;
use egui::scroll_area::ScrollAreaOutput;

pub(super) const WIDTH: f32 = 10.;
pub(super) const MIN_THUMB: f32 = 40.;
pub(super) const MARGIN: f32 = 2.;
pub(super) const GRID_MARGIN: f32 = 10.;
pub(super) const GRID_ALLOCATED_WIDTH: f32 = WIDTH + MARGIN + GRID_MARGIN;

/// One scrollbar treatment for pages, nested lists and dialogs. Content retains
/// its own visuals, so scrollbar hover colors never leak into buttons or labels.
pub(super) struct AppScrollArea {
    area: egui::ScrollArea,
    theme: Theme,
    inner_margin: f32,
}
impl AppScrollArea {
    pub fn vertical(theme: &Theme) -> Self {
        Self {
            area: egui::ScrollArea::vertical(),
            inner_margin: MARGIN,
            theme: theme.clone(),
        }
    }
    pub fn horizontal(theme: &Theme) -> Self {
        Self {
            area: egui::ScrollArea::horizontal(),
            inner_margin: MARGIN,
            theme: theme.clone(),
        }
    }
    pub fn inner_margin(mut self, margin: f32) -> Self {
        self.inner_margin = margin;
        self
    }
    pub fn id_salt(mut self, id: impl std::hash::Hash) -> Self {
        self.area = self.area.id_salt(id);
        self
    }
    pub fn max_height(mut self, height: f32) -> Self {
        self.area = self.area.max_height(height);
        self
    }
    pub fn auto_shrink(mut self, shrink: [bool; 2]) -> Self {
        self.area = self.area.auto_shrink(shrink);
        self
    }
    pub fn show<R>(self, ui: &mut Ui, body: impl FnOnce(&mut Ui) -> R) -> ScrollAreaOutput<R> {
        ui.scope(|ui| {
            let content = ui.visuals().clone();
            apply(ui, &self.theme);
            ui.spacing_mut().scroll.bar_inner_margin = self.inner_margin;
            self.area.show(ui, |ui| {
                *ui.visuals_mut() = content;
                body(ui)
            })
        })
        .inner
    }
    pub fn show_rows<R>(
        self,
        ui: &mut Ui,
        height: f32,
        count: usize,
        body: impl FnOnce(&mut Ui, std::ops::Range<usize>) -> R,
    ) -> ScrollAreaOutput<R> {
        ui.scope(|ui| {
            let content = ui.visuals().clone();
            apply(ui, &self.theme);
            ui.spacing_mut().scroll.bar_inner_margin = self.inner_margin;
            self.area.show_rows(ui, height, count, |ui, range| {
                *ui.visuals_mut() = content;
                body(ui, range)
            })
        })
        .inner
    }
}
fn apply(ui: &mut Ui, theme: &Theme) {
    apply_style(ui.style_mut(), theme);
}
fn apply_style(style: &mut egui::Style, theme: &Theme) {
    let scroll = &mut style.spacing.scroll;
    scroll.floating = false;
    scroll.foreground_color = false;
    scroll.bar_width = WIDTH;
    scroll.handle_min_length = MIN_THUMB;
    scroll.bar_inner_margin = MARGIN;
    scroll.bar_outer_margin = MARGIN;
    let visuals = &mut style.visuals;
    visuals.extreme_bg_color = color(theme.card);
    visuals.widgets.inactive.bg_fill = color(theme.muted);
    visuals.widgets.hovered.bg_fill = color(theme.text);
    visuals.widgets.active.bg_fill = color(theme.text);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
    ] {
        widget.corner_radius = CornerRadius::same(4);
    }
}

/// ComboBox owns an internal ScrollArea, so style its popup before that area
/// is created, then restore normal visuals inside the option list.
pub(super) struct AppComboBox {
    combo: egui::ComboBox,
    selected_text: WidgetText,
    theme: Theme,
}
impl AppComboBox {
    pub fn new(theme: &Theme, id: impl std::hash::Hash) -> Self {
        Self {
            combo: egui::ComboBox::from_id_salt(id),
            selected_text: WidgetText::default(),
            theme: theme.clone(),
        }
    }
    pub fn width(mut self, width: f32) -> Self {
        self.combo = self.combo.width(width);
        self
    }
    pub fn selected_text(mut self, text: impl Into<WidgetText>) -> Self {
        self.selected_text = text.into();
        self
    }
    pub fn show_ui<R>(
        self,
        ui: &mut Ui,
        body: impl FnOnce(&mut Ui) -> R,
    ) -> InnerResponse<Option<R>> {
        let content = ui.visuals().clone();
        let theme = self.theme;
        let popup_theme = theme.clone();
        ui.scope(|ui| {
            buttons::compact_style(ui);
            // Center the label and arrow inside the shared 32-point button.
            let content_height = self
                .selected_text
                .clone()
                .into_galley(
                    ui,
                    Some(TextWrapMode::Extend),
                    f32::INFINITY,
                    TextStyle::Button,
                )
                .size()
                .y
                .max(ui.spacing().icon_width);
            ui.spacing_mut().button_padding.y =
                ((COMPACT_BUTTON_HEIGHT - content_height) / 2.).max(0.);
            self.combo
                .selected_text(self.selected_text)
                .popup_style(egui::style::StyleModifier::new(move |style| {
                    style_menu_settings(style, &popup_theme);
                    apply_style(style, &popup_theme)
                }))
                .show_ui(ui, |ui| {
                    *ui.visuals_mut() = content;
                    style_menu(ui, &theme);
                    body(ui)
                })
        })
        .inner
    }
}

pub(super) fn thumb_length(track: f32, visible: f32, content: f32) -> f32 {
    (track * visible / content.max(1.))
        .max(MIN_THUMB)
        .min(track)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropdowns_keep_launcher_colors_when_windows_uses_the_other_theme() {
        for theme in [Theme::dark(), Theme::light()] {
            let ctx = Context::default();
            theme.apply(&ctx);
            for system_theme in [egui::Theme::Light, egui::Theme::Dark] {
                let mut bounds = Rect::NOTHING;
                let output = ctx.run(
                    RawInput {
                        system_theme: Some(system_theme),
                        ..Default::default()
                    },
                    |ctx| {
                        CentralPanel::default().show(ctx, |ui| {
                            bounds = AppComboBox::new(&theme, "project-filter-colors")
                                .selected_text("All versions")
                                .show_ui(ui, |_| {})
                                .response
                                .rect;
                        });
                    },
                );
                let fill = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        Shape::Rect(rect) if rect.rect == bounds => Some(rect.fill),
                        _ => None,
                    })
                    .expect("dropdown background rendered");
                assert_eq!(
                    fill,
                    color(theme.card),
                    "launcher {} on Windows {system_theme:?}",
                    theme.name
                );
            }
        }
    }
    #[test]
    fn dropdown_buttons_keep_compact_metrics_in_larger_layouts() {
        for scale in [1., 1.25, 1.5] {
            for height in [32., 40., 48.] {
                let ctx = Context::default();
                let mut bounds = Rect::NOTHING;
                let output = ctx.run(RawInput::default(), |ctx| {
                    CentralPanel::default().show(ctx, |ui| {
                        ui.style_mut().override_font_id = Some(FontId::proportional(14. * scale));
                        ui.spacing_mut().button_padding = vec2(10., 5.) * scale;
                        ui.spacing_mut().interact_size.y = height * scale;
                        let canvas = Canvas {
                            origin: Pos2::ZERO,
                            width: 600.,
                            height: 400.,
                            sx: scale,
                            sy: scale,
                            theme: Theme::dark(),
                        };
                        canvas.button(
                            ui,
                            "neighbor-button",
                            button_slot([300., 10., 180., height]),
                            "Manage categories",
                            None,
                            false,
                        );
                        bounds = AppComboBox::new(&Theme::dark(), "alignment-test")
                            .width(200. * scale)
                            .selected_text("Recently opened")
                            .show_ui(ui, |_| {})
                            .response
                            .rect;
                        assert_eq!(ui.spacing().button_padding, vec2(10., 5.) * scale);
                    });
                });
                assert_eq!(bounds.height(), COMPACT_BUTTON_HEIGHT);
                let text_center = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        Shape::Text(text) if text.galley.text() == "Recently opened" => {
                            Some(text.pos.y + text.galley.size().y / 2.)
                        }
                        _ => None,
                    })
                    .expect("dropdown label rendered");
                let button_text_center = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        Shape::Text(text) if text.galley.text() == "Manage categories" => {
                            Some(text.pos.y + text.galley.size().y / 2.)
                        }
                        _ => None,
                    })
                    .expect("neighbor button label rendered");
                let button_center = (10. + height / 2.) * scale;
                assert!(
                    ((button_text_center - button_center) - (text_center - bounds.center().y))
                        .abs()
                        <= 1.,
                    "buttons and dropdowns must use the same vertical text alignment"
                );
                assert!(
                    (text_center - bounds.center().y).abs() <= 1.,
                    "label center {text_center} must match button center {} at scale {scale}",
                    bounds.center().y
                );
            }
        }
    }
    #[test]
    fn dropdown_thumb_keeps_width_and_theme_colors_on_hover_and_drag() {
        for theme in [Theme::dark(), Theme::light()] {
            let ctx = Context::default();
            ctx.set_visuals(theme.visuals());
            ctx.style_mut(|s| s.animation_time = 0.);
            let mut thumb = None;
            for frame in 0..6 {
                let mut input = RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600., 400.))),
                    ..Default::default()
                };
                if frame >= 3 {
                    let rect: Rect = thumb.expect("popup thumb visible before hover");
                    input.events.push(Event::PointerMoved(rect.center()));
                    if frame == 4 {
                        input.events.push(Event::PointerButton {
                            pos: rect.center(),
                            button: PointerButton::Primary,
                            pressed: true,
                            modifiers: Modifiers::NONE,
                        });
                    }
                }
                let output = ctx.run(input, |ctx| {
                    CentralPanel::default().show(ctx, |ui| {
                        let response = AppComboBox::new(&theme, "test-scroll-combo")
                            .width(160.)
                            .selected_text("Engine")
                            .show_ui(ui, |ui| {
                                assert_eq!(
                                    ui.visuals().widgets.hovered.fg_stroke.color,
                                    color(theme.on_accent)
                                );
                                for row in 0..200 {
                                    let _ = ui.selectable_label(false, format!("Engine {row}"));
                                }
                            })
                            .response;
                        if frame == 0 {
                            egui::Popup::open_id(ctx, response.id.with("popup"));
                        }
                    });
                });
                let expected = color(if frame >= 3 { theme.text } else { theme.muted });
                let found = output.shapes.iter().find_map(|s| match &s.shape {
                    Shape::Rect(r)
                        if r.fill == expected
                            && (r.rect.width() - WIDTH).abs() < 0.1
                            && (r.rect.height() - MIN_THUMB).abs() < 0.1 =>
                    {
                        Some(r.rect)
                    }
                    _ => None,
                });
                if frame >= 2 {
                    assert!(
                        found.is_some(),
                        "stable 10px/40px themed thumb in frame {frame}"
                    );
                }
                if found.is_some() {
                    thumb = found;
                }
            }
        }
    }
    #[test]
    fn ten_thousand_rows_remain_grabbable_without_changing_content_visuals() {
        let ctx = Context::default();
        let theme = Theme::dark();
        ctx.set_visuals(theme.visuals());
        ctx.style_mut(|s| s.animation_time = 0.);
        let mut found_thumb = false;
        for _ in 0..2 {
            let output = ctx.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600., 400.))),
                    ..Default::default()
                },
                |ctx| {
                    CentralPanel::default().show(ctx, |ui| {
                        let original = ui.visuals().widgets.hovered.fg_stroke.color;
                        let mut area = AppScrollArea::vertical(&theme);
                        area.area = area.area.scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                        );
                        area.show_rows(ui, 32., 10_000, |ui, range| {
                            assert_eq!(ui.visuals().widgets.hovered.fg_stroke.color, original);
                            assert!(range.len() < 30, "large lists must stay virtualized");
                            for row in range {
                                ui.label(format!("Item {row}"));
                            }
                        });
                        assert_eq!(ui.visuals().widgets.hovered.fg_stroke.color, original);
                    });
                },
            );
            found_thumb |= output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::epaint::Shape::Rect(r) if r.fill == color(theme.muted) && (r.rect.width()-WIDTH).abs()<0.1 && (r.rect.height()-MIN_THUMB).abs()<0.1)
            });
        }
        assert!(
            found_thumb,
            "10,000-item scrollbar should render a 40px thumb"
        );
        assert_eq!(thumb_length(400., 400., 10_000. * 32.), MIN_THUMB);
        assert_eq!(thumb_length(25., 25., 10_000.), 25.);
    }
}
