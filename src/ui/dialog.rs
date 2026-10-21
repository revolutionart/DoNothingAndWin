use super::*;

/// Shared application dialog treatment. Native file pickers and Epic's hosted sign-in
/// remain owned by Windows and Epic respectively.
pub(super) fn show(
    ctx: &Context,
    theme: &Theme,
    id: &str,
    title: &str,
    subtitle: &str,
    open: &mut bool,
    body: impl FnOnce(&mut Ui),
) {
    show_with_icon(ctx, theme, id, title, subtitle, open, None, body);
}

pub(super) fn show_with_icon(
    ctx: &Context,
    theme: &Theme,
    id: &str,
    title: &str,
    subtitle: &str,
    open: &mut bool,
    icon: Option<TextureId>,
    body: impl FnOnce(&mut Ui),
) {
    let preferred_width: f32 = if id == "engine-options" || id == "install-component-options" {
        940.
    } else {
        680.
    };
    let width = preferred_width.min((ctx.content_rect().width() - 96.).max(280.));
    let mut close = false;
    let response = Modal::new(Id::new(id))
        .backdrop_color(Color32::from_black_alpha(175))
        .frame(
            Frame::new()
                .fill(color(theme.sidebar))
                .stroke(Stroke::NONE)
                .corner_radius(7.)
                .inner_margin(24)
                .shadow(Shadow {
                    offset: [0, 10],
                    blur: 30,
                    spread: 0,
                    color: Color32::from_black_alpha(90),
                }),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.set_max_height((ctx.content_rect().height() - 96.).max(240.));
            ui.style_mut().override_font_id = Some(FontId::proportional(16.));
            ui.spacing_mut().item_spacing = vec2(12., 12.);
            ui.spacing_mut().button_padding = vec2(18., 9.);
            ui.spacing_mut().interact_size.y = 40.;
            ui.spacing_mut().text_edit_width = width;
            let visuals = ui.visuals_mut();
            for widget in [
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.active,
            ] {
                widget.bg_stroke = Stroke::NONE;
                widget.corner_radius = CornerRadius::same(6);
                widget.expansion = 0.;
            }
            visuals.widgets.inactive.bg_stroke = Stroke::new(1., color(theme.border));
            visuals.widgets.inactive.weak_bg_fill = color(theme.card);
            visuals.widgets.hovered.weak_bg_fill = color([234, 189, 93]);
            visuals.widgets.active.weak_bg_fill = color([234, 189, 93]);
            let start = ui.cursor().min;
            let (header, _) = ui.allocate_exact_size(vec2(width, 76.), Sense::hover());
            if let Some(icon) = icon {
                ui.painter().image(
                    icon,
                    Rect::from_min_size(start + vec2(0., 6.), vec2(48., 48.)),
                    Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                    color(theme.logo),
                );
            } else if title == "Install to engine" || title == "Download plugin" {
                let p = start + vec2(6., 14.);
                ui.painter().rect_filled(
                    Rect::from_min_size(p, vec2(25., 25.)),
                    3.,
                    color(theme.text),
                );
                ui.painter()
                    .circle_filled(p + vec2(12., -2.), 6., color(theme.text));
                ui.painter()
                    .circle_filled(p + vec2(27., 12.), 6., color(theme.text));
                ui.painter()
                    .circle_filled(p + vec2(-1., 12.), 5., color(theme.panel));
                ui.painter()
                    .circle_filled(p + vec2(12., 26.), 5., color(theme.panel));
            } else {
                let icon = Rect::from_min_size(start + vec2(3., 9.), vec2(28., 29.));
                ui.painter().rect_stroke(
                    icon,
                    3.,
                    Stroke::new(2., color(theme.text)),
                    StrokeKind::Inside,
                );
                ui.painter()
                    .rect_filled(icon.translate(vec2(7., 7.)), 3., color(theme.panel));
                ui.painter().rect_stroke(
                    icon.translate(vec2(7., 7.)),
                    3.,
                    Stroke::new(2., color(theme.text)),
                    StrokeKind::Inside,
                );
            }
            ui.painter().text(
                start + vec2(62., 22.),
                Align2::LEFT_CENTER,
                title,
                FontId::new(26., FontFamily::Name("semibold".into())),
                color(theme.text),
            );
            let subtitle_text = ui.painter().layout(
                subtitle.to_owned(),
                FontId::proportional(16.),
                color(theme.muted),
                width - 100.,
            );
            ui.painter()
                .galley(start + vec2(62., 45.), subtitle_text, color(theme.muted));
            let x = Rect::from_min_size(pos2(header.right() - 28., header.top()), vec2(28., 28.));
            let hover = ui
                .interact(x, Id::new((id, "close")), Sense::click())
                .on_hover_text("Close");
            let col = color(if hover.hovered() {
                theme.text
            } else {
                theme.muted
            });
            ui.painter().line_segment(
                [x.min + vec2(6., 6.), x.max - vec2(6., 6.)],
                Stroke::new(2., col),
            );
            ui.painter().line_segment(
                [
                    pos2(x.left() + 6., x.bottom() - 6.),
                    pos2(x.right() - 6., x.top() + 6.),
                ],
                Stroke::new(2., col),
            );
            close = hover
                .clone()
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked();
            AppScrollArea::vertical(theme)
                .id_salt((id, "body"))
                .max_height((ctx.content_rect().height() - 200.).max(160.))
                .auto_shrink([false, true])
                .show(ui, body);
        });
    if close || response.should_close() {
        *open = false;
    }
}

pub(super) fn footer(ui: &mut Ui, theme: &Theme, label: &str, enabled: bool, danger: bool) -> bool {
    ui.add_space(8.);
    ui.separator();
    ui.add_space(4.);
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), COMPACT_BUTTON_HEIGHT),
        Layout::right_to_left(Align::Center),
        |ui| {
            let action = if danger {
                ui.scope(|ui| {
                    ui.spacing_mut().button_padding = vec2(buttons::HORIZONTAL_PADDING, 5.);
                    ui.spacing_mut().interact_size.y = COMPACT_BUTTON_HEIGHT;
                    let fill = color(if enabled { theme.danger } else { theme.button });
                    let widgets = &mut ui.visuals_mut().widgets;
                    for widget in [
                        &mut widgets.inactive,
                        &mut widgets.hovered,
                        &mut widgets.active,
                        &mut widgets.noninteractive,
                    ] {
                        widget.weak_bg_fill = fill;
                    }
                    ui.add_enabled(
                        enabled,
                        buttons::secondary_button(
                            RichText::new(label)
                                .font(buttons::primary_font(1.))
                                .color(color(theme.on_accent)),
                        )
                        .stroke(Stroke::NONE)
                        .corner_radius(BUTTON_RADIUS)
                        .min_size(vec2(150., COMPACT_BUTTON_HEIGHT)),
                    )
                })
                .inner
            } else {
                ui.add_enabled(
                    enabled,
                    primary_button(label, theme).min_size(vec2(150., COMPACT_BUTTON_HEIGHT)),
                )
            }
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked();
            let cancel = ui
                .scope(|ui| {
                    ui.spacing_mut().button_padding = vec2(buttons::HORIZONTAL_PADDING, 5.);
                    ui.spacing_mut().interact_size.y = COMPACT_BUTTON_HEIGHT;
                    ui.add(
                        buttons::secondary_button(
                            RichText::new("Cancel")
                                .font(FontId::proportional(COMPACT_BUTTON_FONT_SIZE)),
                        )
                        .min_size(vec2(120., COMPACT_BUTTON_HEIGHT))
                        .corner_radius(BUTTON_RADIUS),
                    )
                })
                .inner;
            if cancel.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                ui.close();
            }
            action
        },
    )
    .inner
}

pub(super) fn info(ui: &mut Ui, theme: &Theme, label: &str, value: &str) {
    Frame::new()
        .fill(color(theme.card))
        .stroke(Stroke::new(1., color(theme.border)))
        .corner_radius(6.)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width((ui.available_width() - 2.).max(100.));
            ui.label(RichText::new(label).color(color(theme.muted)));
            ui.add(Label::new(RichText::new(display_path(value)).monospace().size(14.)).wrap());
        });
}

pub(super) fn folder(ui: &mut Ui, value: &mut String) {
    ui.horizontal(|ui| {
        path_input(ui, value, ui.available_width() - 112.);
        if ui
            .add_sized(
                [100., COMPACT_BUTTON_HEIGHT],
                buttons::secondary_button("Browse"),
            )
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new().set_directory(&*value).pick_folder() {
                *value = path.to_string_lossy().into_owned();
            }
        }
    });
}

// Keep canonical paths for filesystem operations; remove Windows device syntax only in UI.
pub(super) fn display_path(value: &str) -> String {
    if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", unc)
    } else {
        value.strip_prefix(r"\\?\").unwrap_or(value).to_owned()
    }
}

pub(super) fn path_input(ui: &mut Ui, value: &mut String, width: f32) -> Response {
    let mut display = display_path(value);
    let response = ui.add_sized(
        [width, 40.],
        TextEdit::singleline(&mut display)
            .margin(vec2(12., 8.))
            .vertical_align(Align::Center),
    );
    if response.changed() {
        *value = display;
    }
    response
}

#[cfg(test)]
mod tests {
    use super::display_path;

    #[test]
    fn display_windows_paths_without_device_prefix() {
        assert_eq!(
            display_path(r"\\?\C:\Projects\Example"),
            r"C:\Projects\Example"
        );
        assert_eq!(
            display_path(r"\\?\UNC\server\share\Example"),
            r"\\server\share\Example"
        );
        assert_eq!(display_path(r"F:\Unreal Projects"), r"F:\Unreal Projects");
    }
}
