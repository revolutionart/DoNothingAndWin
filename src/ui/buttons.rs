use super::*;

// All action buttons share these metrics in both egui layouts and painted cards.
pub(super) const HORIZONTAL_PADDING: f32 = 14.;

/// Shared update-action palette for Library and engine cards.
pub(super) fn update_colors(theme: &Theme, hovered: bool) -> ([u8; 3], [u8; 3]) {
    let contrast = if theme.dark {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    let fill = if hovered {
        color(theme.warning).lerp_to_gamma(contrast, 0.15)
    } else {
        color(theme.warning)
    };
    let [r, g, b, _] = fill.to_array();
    (
        [r, g, b],
        if theme.dark {
            [0, 0, 0]
        } else {
            [255, 255, 255]
        },
    )
}

const CHECKBOX_SIZE: f32 = 18.;
const CHECKBOX_RADIUS: u8 = 3;

pub(super) struct AppCheckbox<'a> {
    checked: &'a mut bool,
    label: String,
    theme: &'a Theme,
}

pub(super) fn checkbox<'a>(
    checked: &'a mut bool,
    label: impl Into<String>,
    theme: &'a Theme,
) -> AppCheckbox<'a> {
    AppCheckbox {
        checked,
        label: label.into(),
        theme,
    }
}

impl Widget for AppCheckbox<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let gap = if self.label.is_empty() { 0. } else { 8. };
        let text: WidgetText = RichText::new(&self.label)
            .color(color(self.theme.text))
            .into();
        let galley = text.into_galley(
            ui,
            Some(egui::TextWrapMode::Wrap),
            (ui.available_width() - CHECKBOX_SIZE - gap).max(0.),
            TextStyle::Button,
        );
        let size = vec2(
            CHECKBOX_SIZE + gap + galley.size().x,
            CHECKBOX_SIZE
                .max(galley.size().y)
                .max(ui.spacing().interact_size.y),
        );
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
        if response.clicked() {
            *self.checked = !*self.checked;
            response.mark_changed();
        }
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::Checkbox,
                ui.is_enabled(),
                *self.checked,
                &self.label,
            )
        });
        if ui.is_rect_visible(rect) {
            let indicator = Rect::from_center_size(
                pos2(rect.left() + CHECKBOX_SIZE / 2., rect.center().y),
                Vec2::splat(CHECKBOX_SIZE),
            );
            let highlighted = response.hovered() || response.is_pointer_button_down_on();
            let fill = color(if *self.checked {
                if highlighted {
                    self.theme.accent_hover
                } else {
                    self.theme.accent
                }
            } else {
                self.theme.card
            });
            let border = if *self.checked {
                fill
            } else {
                color(if highlighted || response.has_focus() {
                    self.theme.accent
                } else {
                    self.theme.border
                })
            };
            ui.painter().rect(
                indicator,
                CornerRadius::same(CHECKBOX_RADIUS),
                fill,
                Stroke::new(1., border),
                StrokeKind::Inside,
            );
            if *self.checked {
                ui.painter().add(Shape::line(
                    vec![
                        indicator.min + vec2(4., 9.),
                        indicator.min + vec2(8., 13.),
                        indicator.min + vec2(14., 5.),
                    ],
                    Stroke::new(2., color(self.theme.on_accent)),
                ));
            }
            if response.has_focus() {
                ui.painter().rect_stroke(
                    indicator.expand(2.),
                    CornerRadius::same(CHECKBOX_RADIUS + 1),
                    Stroke::new(1., color(self.theme.accent)),
                    StrokeKind::Outside,
                );
            }
            // Labels remain on the page surface and never inherit indicator colors.
            ui.painter().galley(
                pos2(
                    indicator.right() + gap,
                    rect.center().y - galley.size().y / 2.,
                ),
                galley,
                color(self.theme.text),
            );
        }
        response
    }
}

pub(super) fn compact_style(ui: &mut Ui) {
    ui.spacing_mut().button_padding = vec2(HORIZONTAL_PADDING, 5.);
    ui.spacing_mut().interact_size.y = COMPACT_BUTTON_HEIGHT;
    ui.style_mut().override_font_id = Some(FontId::proportional(COMPACT_BUTTON_FONT_SIZE));
}

pub(super) struct SecondaryButton<'a>(Button<'a>);
impl SecondaryButton<'_> {
    pub(super) fn min_size(mut self, size: Vec2) -> Self {
        self.0 = self.0.min_size(vec2(size.x, COMPACT_BUTTON_HEIGHT));
        self
    }
    pub(super) fn stroke(mut self, stroke: Stroke) -> Self {
        self.0 = self.0.stroke(stroke);
        self
    }
    pub(super) fn corner_radius(mut self, radius: impl Into<CornerRadius>) -> Self {
        self.0 = self.0.corner_radius(radius);
        self
    }
}
impl Widget for SecondaryButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.scope(|ui| {
            compact_style(ui);
            ui.add(self.0)
        })
        .inner
    }
}
pub(super) fn secondary_button<'a>(label: impl Into<WidgetText>) -> SecondaryButton<'a> {
    SecondaryButton(
        Button::new(label)
            .min_size(vec2(0., COMPACT_BUTTON_HEIGHT))
            .corner_radius(BUTTON_RADIUS),
    )
}
pub(super) fn primary_font(scale: f32) -> FontId {
    FontId::new(
        COMPACT_BUTTON_FONT_SIZE * scale,
        FontFamily::Name("semibold".into()),
    )
}
pub(super) fn primary_fill(theme: &Theme, response: &Response) -> Color32 {
    color(if !response.enabled() {
        theme.button
    } else if response.hovered() || response.is_pointer_button_down_on() {
        theme.accent_hover
    } else {
        theme.accent
    })
}

pub(super) struct PrimaryButton<'a> {
    label: String,
    theme: &'a Theme,
    width: f32,
}
impl PrimaryButton<'_> {
    pub(super) fn min_size(mut self, size: Vec2) -> Self {
        self.width = size.x;
        self
    }
}
impl Widget for PrimaryButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.scope(|ui| {
            // Dialogs and pages may have larger body text and control spacing.
            // Explicit metrics keep primary actions independent of that context.
            ui.spacing_mut().button_padding = vec2(HORIZONTAL_PADDING, 5.);
            ui.spacing_mut().interact_size.y = COMPACT_BUTTON_HEIGHT;
            let ui_is_enabled = ui.is_enabled();
            let widgets = &mut ui.visuals_mut().widgets;
            for widget in [&mut widgets.inactive, &mut widgets.noninteractive] {
                widget.bg_fill = color(if ui_is_enabled {
                    self.theme.accent
                } else {
                    self.theme.button
                });
                widget.weak_bg_fill = widget.bg_fill;
            }
            for widget in [&mut widgets.hovered, &mut widgets.active] {
                widget.bg_fill = color(self.theme.accent_hover);
                widget.weak_bg_fill = widget.bg_fill;
            }
            ui.add(
                Button::new(
                    RichText::new(self.label)
                        .font(primary_font(1.))
                        .color(color(self.theme.on_accent)),
                )
                .min_size(vec2(self.width, COMPACT_BUTTON_HEIGHT))
                .corner_radius(BUTTON_RADIUS)
                .stroke(Stroke::NONE),
            )
        })
        .inner
    }
}
pub(super) fn primary_button(label: impl Into<String>, theme: &Theme) -> PrimaryButton<'_> {
    PrimaryButton {
        label: label.into(),
        theme,
        width: 0.,
    }
}

pub(super) fn tab_button(
    ui: &mut Ui,
    theme: &Theme,
    selected: bool,
    label: impl Into<String>,
    width: f32,
) -> Response {
    let label = label.into();
    let response = if selected {
        ui.add(primary_button(&label, theme).min_size(vec2(width, COMPACT_BUTTON_HEIGHT)))
    } else {
        ui.scope(|ui| {
            ui.spacing_mut().button_padding = vec2(HORIZONTAL_PADDING, 5.);
            ui.spacing_mut().interact_size.y = COMPACT_BUTTON_HEIGHT;
            ui.add(
                Button::new(
                    RichText::new(&label)
                        .font(primary_font(1.))
                        .color(color(theme.text)),
                )
                .fill(color(theme.background))
                .stroke(Stroke::NONE)
                .corner_radius(BUTTON_RADIUS)
                .min_size(vec2(width, COMPACT_BUTTON_HEIGHT)),
            )
        })
        .inner
    };
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            response.enabled(),
            selected,
            &label,
        )
    });
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        let ctx = Context::default();
        let mut fonts = FontDefinitions::default();
        fonts.families.insert(
            FontFamily::Name("semibold".into()),
            fonts.families[&FontFamily::Proportional].clone(),
        );
        ctx.set_fonts(fonts);
        ctx
    }

    #[test]
    fn checkbox_labels_keep_surface_text_color_when_hovered_and_pressed() {
        for theme in [Theme::dark(), Theme::light()] {
            for initial in [false, true] {
                let ctx = context();
                theme.apply(&ctx);
                let mut checked = initial;
                let mut bounds = Rect::NOTHING;
                for frame in 0..4 {
                    let mut input = RawInput::default();
                    if frame > 0 {
                        input.events.push(Event::PointerMoved(bounds.center()));
                    }
                    if frame >= 2 {
                        input.events.push(Event::PointerButton {
                            pos: bounds.center(),
                            button: PointerButton::Primary,
                            pressed: frame == 2,
                            modifiers: Modifiers::NONE,
                        });
                    }
                    let output = ctx.run(input, |ctx| {
                        CentralPanel::default().show(ctx, |ui| {
                            let response =
                                ui.add(checkbox(&mut checked, "Show hidden (11)", &theme));
                            bounds = response.rect;
                            if frame > 0 {
                                assert!(response.hovered());
                            }
                            if frame == 2 {
                                assert!(response.is_pointer_button_down_on());
                            }
                        });
                    });
                    let indicator = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            Shape::Rect(shape) if shape.rect.size() == Vec2::splat(18.) => {
                                Some(shape)
                            }
                            _ => None,
                        })
                        .expect("larger checkbox indicator rendered");
                    assert_eq!(indicator.corner_radius, CornerRadius::same(3));
                    assert_eq!(
                        indicator.fill,
                        color(if checked {
                            if frame > 0 {
                                theme.accent_hover
                            } else {
                                theme.accent
                            }
                        } else {
                            theme.card
                        })
                    );
                    if checked {
                        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                            Shape::Path(path) if path.points.len() == 3 && path.stroke.color == egui::epaint::ColorMode::Solid(color(theme.on_accent)))));
                    }
                    let text = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            Shape::Text(text) if text.galley.text() == "Show hidden (11)" => {
                                Some(text)
                            }
                            _ => None,
                        })
                        .expect("checkbox label rendered");
                    let section_color = text.galley.job.sections[0].format.color;
                    let actual = text.override_text_color.unwrap_or(
                        if section_color == Color32::PLACEHOLDER {
                            text.fallback_color
                        } else {
                            section_color
                        },
                    );
                    assert_eq!(
                        actual,
                        color(theme.text),
                        "label must contrast with the surface on frame {frame}"
                    );
                }
                assert_eq!(checked, !initial, "click must still toggle the checkbox");
            }
        }
    }

    #[test]
    fn primary_metrics_ignore_page_and_dialog_body_styles() {
        for (body_font, padding, control_height) in
            [(14., 5., 32.), (16., 9., 40.), (20., 12., 48.)]
        {
            let ctx = context();
            let theme = Theme::dark();
            let output = ctx.run(RawInput::default(), |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    ui.style_mut().override_font_id = Some(FontId::proportional(body_font));
                    ui.spacing_mut().button_padding = Vec2::splat(padding);
                    ui.spacing_mut().interact_size.y = control_height;
                    let response = ui.add(primary_button("Install to engine", &theme));
                    assert_eq!(response.rect.height(), 32.);
                    assert_eq!(ui.spacing().interact_size.y, control_height);
                    assert_eq!(ui.spacing().button_padding, Vec2::splat(padding));
                });
            });
            let text = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Text(text) => Some(text),
                    _ => None,
                })
                .expect("primary label rendered");
            assert_eq!(text.galley.job.sections[0].format.font_id, primary_font(1.));
        }
    }

    #[test]
    fn secondary_metrics_ignore_page_and_dialog_body_styles() {
        for (body_font, padding, control_height) in
            [(14., 5., 32.), (16., 9., 40.), (20., 12., 48.)]
        {
            let ctx = context();
            let output = ctx.run(RawInput::default(), |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    ui.style_mut().override_font_id = Some(FontId::proportional(body_font));
                    ui.spacing_mut().button_padding = Vec2::splat(padding);
                    ui.spacing_mut().interact_size.y = control_height;
                    let response = ui.add(secondary_button("Browse").min_size(vec2(100., 40.)));
                    assert_eq!(response.rect.height(), 32.);
                    assert_eq!(ui.spacing().interact_size.y, control_height);
                    assert_eq!(ui.spacing().button_padding, Vec2::splat(padding));
                });
            });
            let text = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Text(text) => Some(text),
                    _ => None,
                })
                .expect("secondary label rendered");
            assert_eq!(
                text.galley.job.sections[0].format.font_id.size,
                COMPACT_BUTTON_FONT_SIZE
            );
        }
    }

    #[test]
    fn primary_hover_and_disabled_states_survive_dialog_styling() {
        let ctx = context();
        let theme = Theme::dark();
        let mut rect = Rect::NOTHING;
        for frame in 0..3 {
            let mut input = RawInput::default();
            if frame > 0 {
                input.events.push(Event::PointerMoved(rect.center()));
            }
            let enabled = frame < 2;
            let output = ctx.run(input, |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    *ui.visuals_mut() = theme.visuals();
                    let response = ui.add_enabled(enabled, primary_button("Install", &theme));
                    rect = response.rect;
                    assert!(!response.clicked());
                    assert_eq!(response.enabled(), enabled);
                });
            });
            if frame < 2 {
                let expected = color(if frame == 1 {
                    theme.accent_hover
                } else {
                    theme.accent
                });
                assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                    Shape::Rect(r) if r.rect == rect && r.fill == expected)));
            }
        }
    }

    #[test]
    fn painted_primary_buttons_match_layout_buttons_with_or_without_icons() {
        for icon in [None, Some(Icon::Book)] {
            let ctx = context();
            let theme = Theme::dark();
            let output = ctx.run(RawInput::default(), |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    let canvas = Canvas {
                        origin: Pos2::ZERO,
                        width: 600.,
                        height: 400.,
                        sx: 1.,
                        sy: 1.,
                        theme: theme.clone(),
                    };
                    canvas.button(
                        ui,
                        "primary",
                        button_slot([40., 40., 180., 40.]),
                        "Install",
                        icon,
                        true,
                    );
                });
            });
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                Shape::Rect(r) if r.fill == color(theme.accent) && (r.rect.height() - 32.).abs() < 0.01)));
            let text = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Text(text) => Some(text),
                    _ => None,
                })
                .expect("painted label rendered");
            assert_eq!(text.galley.job.sections[0].format.font_id, primary_font(1.));
        }
    }
}
