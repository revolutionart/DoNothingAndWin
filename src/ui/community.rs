use super::*;

const LINKS: &[(Icon, &str, &str)] = &[
    (
        Icon::Discord,
        "Discord support",
        "https://discord.gg/DPuGf4CCX",
    ),
    (
        Icon::Github,
        "GitHub repository",
        "https://github.com/revolutionart/DoNothingAndWin",
    ),
];

impl Workspace {
    pub(super) fn account_community_row(&mut self, ui: &mut Ui, theme: &Theme) {
        let gap = ui.spacing().item_spacing.x;
        let width = ui.available_width();
        let account_width = (width - gap) / 2.;
        let link_width = (width - account_width - 2. * gap) / 2.;
        ui.horizontal_top(|ui| {
            let account = ui.allocate_ui_with_layout(vec2(account_width,0.),Layout::top_down(Align::Min),|ui| {
                ui.set_width(account_width);
                settings_flow::settings_section(ui,theme,|ui| {
                    ui.heading("Epic account");
                    ui.horizontal_wrapped(|ui| {
                        ui.label(self.session.as_ref().map(|s|s.display_name.as_str()).unwrap_or("Not connected"));
                        if self.session.is_some() {
                            if ui.add(buttons::secondary_button("Sign out")).clicked() { self.sign_out_account(); }
                        } else if ui.add(buttons::secondary_button("Sign in with Epic")).clicked() { self.login = true; }
                    });
                    ui.label("Each developer signs in with their own account. Sessions are encrypted for this Windows user.");
                })
            });
            for (icon,label,url) in LINKS {
                let response = buttons::link_card(ui,theme,vec2(link_width,account.inner.rect.height()),*icon,label).on_hover_text(*label);
                if response.clicked() { let _ = platform::open_external(url); }
            }
        });
    }
}

pub(super) fn draw_brand_icon(
    p: &Painter,
    icon: Icon,
    point: &impl Fn(f32, f32) -> Pos2,
    s: Stroke,
) {
    let outline: &[[f32; 2]] = match icon {
        Icon::Discord => &[
            [7., 4.],
            [3.5, 5.],
            [1., 17.],
            [6., 20.],
            [8., 17.5],
            [16., 17.5],
            [18., 20.],
            [23., 17.],
            [20.5, 5.],
            [17., 4.],
            [16., 6.],
            [8., 6.],
        ],
        _ => &[
            [7., 21.],
            [7., 18.],
            [5., 18.],
            [3., 16.],
            [2., 16.],
            [4., 19.],
            [7., 19.],
            [8., 17.],
            [6., 16.],
            [4., 14.],
            [3.5, 10.],
            [5., 7.],
            [5., 3.],
            [9., 5.],
            [15., 5.],
            [19., 3.],
            [19., 7.],
            [20.5, 10.],
            [20., 14.],
            [18., 16.],
            [15., 17.],
            [16., 19.],
            [16., 22.],
        ],
    };
    p.add(Shape::closed_line(
        outline.iter().map(|v| point(v[0], v[1])).collect(),
        s,
    ));
    if matches!(icon, Icon::Discord) {
        for x in [8., 16.] {
            p.circle_filled(
                point(x, 12.),
                1.5 * (point(1., 0.).x - point(0., 0.).x).abs(),
                s.color,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn support_icons_use_shared_hit_boxes_and_center_in_both_themes_and_scales() {
        for theme in [Theme::dark(), Theme::light()] {
            for scale in [1., 1.25] {
                for (icon, label, _) in LINKS {
                    let ctx = Context::default();
                    let mut bounds = Rect::NOTHING;
                    let output = ctx.run(RawInput::default(), |ctx| {
                        CentralPanel::default().show(ctx, |ui| {
                            let canvas = Canvas {
                                origin: Pos2::ZERO,
                                width: 1000.,
                                height: 680.,
                                sx: scale,
                                sy: scale,
                                theme: theme.clone(),
                            };
                            bounds = canvas
                                .icon_button_response(
                                    ui,
                                    label,
                                    [24., 566., 88., 32.],
                                    *icon,
                                    false,
                                )
                                .rect;
                        });
                    });
                    assert!((bounds.height() - 32. * scale).abs() < 0.01);
                    let mut ink = Rect::NOTHING;
                    let mut icon_shapes = Vec::new();
                    for clipped in output.shapes {
                        if matches!(clipped.shape, Shape::Path(_) | Shape::Circle(_)) {
                            ink = ink.union(clipped.shape.visual_bounding_rect());
                            icon_shapes.push(clipped);
                        }
                    }
                    assert!(bounds.contains_rect(ink));
                    assert!((bounds.center().y - ink.center().y).abs() < 2. * scale);
                    for primitive in ctx.tessellate(icon_shapes, output.pixels_per_point) {
                        if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                            for vertex in mesh.vertices {
                                assert!(
                                    bounds.expand(1.).contains(vertex.pos),
                                    "Icon stroke escaped its button: {:?} outside {:?}",
                                    vertex.pos,
                                    bounds
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
