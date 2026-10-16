use super::*;

// All action buttons share these metrics in both egui layouts and painted cards.
pub(super) const HORIZONTAL_PADDING: f32 = 14.;
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
