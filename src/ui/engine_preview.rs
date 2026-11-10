use super::*;

pub(super) const RELEASE_NOTES_URL: &str =
    "https://forums.unrealengine.com/tags/c/general/announcements/49/unreal-engine/705";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    Dismiss,
    Install,
    ReleaseNotes,
}

fn stacks_actions(width: f32) -> bool {
    width < 1170.
}

pub(super) fn banner_height(width: f32) -> f32 {
    if stacks_actions(width) { 134. } else { 104. }
}

pub(super) fn banner(ui: &mut Ui, c: &Canvas, y: f32, branch: &str) -> Option<Action> {
    let x = 271.;
    let right = c.width - 26.;
    let height = banner_height(c.width);
    let warning = c.theme.warning;
    let tint = |strength| {
        let [r, g, b, _] = color(c.theme.card)
            .lerp_to_gamma(color(warning), strength)
            .to_array();
        [r, g, b]
    };
    c.boxed(ui, [x, y, right - x, height], tint(0.08), warning, 8., 1.);
    c.fill(ui, [x + 24., y + 22., 56., 56.], tint(0.16), 9.);
    let flask_x = x + 38.;
    let flask_y = y + 35.;
    let flask = [
        [10., 0.],
        [18., 0.],
        [17., 2.],
        [17., 10.],
        [26., 25.],
        [24., 28.],
        [4., 28.],
        [2., 25.],
        [11., 10.],
        [11., 2.],
        [10., 0.],
    ];
    ui.painter().add(Shape::line(
        flask
            .into_iter()
            .map(|[x, y]| c.point(flask_x + x, flask_y + y))
            .collect(),
        Stroke::new(1.8 * c.scale(), color(warning)),
    ));
    c.circle(ui, flask_x + 13., flask_y + 22., 1.3, warning);
    c.circle(ui, flask_x + 16., flask_y + 17., 1.1, warning);

    let text_x = x + 104.;
    c.text(
        ui,
        text_x,
        y + 24.,
        "NEW PREVIEW",
        14.,
        true,
        warning,
        Align2::LEFT_CENTER,
    );
    c.text(
        ui,
        text_x,
        y + 49.,
        &format!("Unreal Engine {branch} Preview is available"),
        23.,
        true,
        c.theme.text,
        Align2::LEFT_CENTER,
    );
    c.text(
        ui,
        text_x,
        y + 76.,
        "Try the preview in a separate installation.",
        16.,
        false,
        c.theme.muted,
        Align2::LEFT_CENTER,
    );

    let dismiss = ui
        .interact(
            c.rect([right - 34., y + 5., 28., 28.]),
            Id::new("dismiss-engine-preview"),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Dismiss this preview banner");
    dismiss.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Dismiss preview banner"));
    let close_color = if dismiss.hovered() {
        c.theme.text
    } else {
        c.theme.muted
    };
    let close_x = right - 20.;
    let close_y = y + 19.;
    c.line(
        ui,
        [close_x - 5., close_y - 5.],
        [close_x + 5., close_y + 5.],
        close_color,
    );
    c.line(
        ui,
        [close_x + 5., close_y - 5.],
        [close_x - 5., close_y + 5.],
        close_color,
    );

    let action_y = if stacks_actions(c.width) {
        y + 111.
    } else {
        y + height / 2.
    };
    let mut gold = c.theme.clone();
    gold.accent = gold.launch_background;
    gold.accent_hover = gold.launch_hover;
    gold.on_accent = gold.launch_foreground;
    let gold_canvas = Canvas { theme: gold, ..*c };
    let install = gold_canvas.button(
        ui,
        "install-engine-preview",
        button_slot([right - 294., action_y - 16., 150., 32.]),
        "Install preview",
        None,
        true,
    );
    let notes = ui
        .interact(
            c.rect([right - 134., action_y - 16., 104., 32.]),
            Id::new("engine-preview-release-notes"),
            Sense::click(),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    notes.widget_info(|| WidgetInfo::labeled(WidgetType::Link, true, "Release notes"));
    c.control_text(
        ui,
        right - 82.,
        action_y,
        "Release notes",
        false,
        if notes.hovered() {
            c.theme.text
        } else {
            c.theme.muted
        },
        Align2::CENTER_CENTER,
    );

    if dismiss.clicked() {
        Some(Action::Dismiss)
    } else if install {
        Some(Action::Install)
    } else if notes.clicked() {
        Some(Action::ReleaseNotes)
    } else {
        None
    }
}

pub(super) fn compact_notice(ui: &mut Ui, c: &Canvas, x: f32, branch: &str) -> bool {
    let label = format!("UE {branch} Preview Available");
    ui.painter().circle_stroke(
        c.point(x + 8., 144.),
        8.,
        Stroke::new(1.5, color(c.theme.warning)),
    );
    c.text(
        ui,
        x + 8.,
        144.,
        "!",
        13.,
        true,
        c.theme.warning,
        Align2::CENTER_CENTER,
    );
    let text = ui.painter().text(
        c.point(x + 24., 144.),
        Align2::LEFT_CENTER,
        &label,
        buttons::primary_font(c.scale()),
        color(c.theme.warning),
    );
    let notice = ui.interact(
        Rect::from_min_max(
            c.point(x - 2., 130.),
            pos2(text.right() + 4., c.point(0., 158.).y),
        ),
        Id::new("workspace-engine-preview"),
        Sense::click(),
    );
    notice.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
    notice
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Show the preview install card")
        .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_actions_are_separate_in_compact_and_wide_layouts() {
        for width in [1000., 1400.] {
            let y = 116.;
            let right = width - 26.;
            let action_y = y + if width < 1170. { 111. } else { 52. };
            for (point, expected) in [
                (pos2(right - 20., y + 19.), Action::Dismiss),
                (pos2(right - 219., action_y), Action::Install),
                (pos2(right - 82., action_y), Action::ReleaseNotes),
            ] {
                let ctx = Context::default();
                let mut fonts = FontDefinitions::default();
                fonts.families.insert(
                    FontFamily::Name("semibold".into()),
                    fonts.families[&FontFamily::Proportional].clone(),
                );
                ctx.set_fonts(fonts);
                let c = Canvas {
                    origin: Pos2::ZERO,
                    width,
                    height: 680.,
                    sx: 1.,
                    sy: 1.,
                    theme: Theme::dark(),
                };
                let mut result = None;
                for pressed in [None, Some(true), Some(false)] {
                    let events = pressed.map_or_else(Vec::new, |pressed| {
                        vec![
                            Event::PointerMoved(point),
                            Event::PointerButton {
                                pos: point,
                                button: PointerButton::Primary,
                                pressed,
                                modifiers: Modifiers::NONE,
                            },
                        ]
                    });
                    let _ = ctx.run(
                        RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                vec2(width, c.height),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
                                result = banner(ui, &c, y, "5.9");
                            });
                        },
                    );
                }
                assert_eq!(result, Some(expected));
            }
        }
    }
}
