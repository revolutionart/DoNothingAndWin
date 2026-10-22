use super::*;

pub(super) const SIZE: f32 = 16.;

/// All circular loading indicators use this fixed geometry and contrast.
/// Callers choose placement, never size or color.
pub(super) fn spinner(theme: &Theme) -> Loader {
    Loader {
        color: if theme.dark {
            Color32::WHITE
        } else {
            color(theme.text)
        },
    }
}
pub(super) struct Loader {
    color: Color32,
}
impl Widget for Loader {
    fn ui(self, ui: &mut Ui) -> Response {
        let (slot, response) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::hover());
        response.widget_info(|| WidgetInfo::new(WidgetType::ProgressIndicator));
        // ui.put may justify a widget into a larger slot. Paint the same small
        // circle at its center even in a full-size button or dialog overlay.
        if ui.is_rect_visible(slot) {
            ui.ctx().request_repaint();
            let start = ui.input(|i| i.time) as f32 * std::f32::consts::TAU;
            let radius = SIZE / 2. - 2.;
            let points = (0..=24)
                .map(|index| {
                    let angle = start + index as f32 / 24. * 240_f32.to_radians();
                    slot.center() + radius * vec2(angle.cos(), angle.sin())
                })
                .collect();
            ui.painter()
                .add(Shape::line(points, Stroke::new(3., self.color)));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn animation_keeps_the_same_arc_instead_of_shrinking_to_a_dot() {
        for time in [0., 0.25, 1., std::f64::consts::PI] {
            let ctx = Context::default();
            let mut center = Pos2::ZERO;
            let output = ctx.run(
                RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    CentralPanel::default().show(ctx, |ui| {
                        center = ui.add(spinner(&Theme::dark())).rect.center();
                    });
                },
            );
            let path = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    Shape::Path(p) => Some(p),
                    _ => None,
                })
                .unwrap();
            let sweep: f32 = path
                .points
                .windows(2)
                .map(|points| {
                    let a = points[0] - center;
                    let b = points[1] - center;
                    (a.x * b.y - a.y * b.x).atan2(a.dot(b))
                })
                .sum();
            assert!(
                (sweep - 240_f32.to_radians()).abs() < 0.01,
                "arc collapsed at {time}"
            );
        }
    }
    #[test]
    fn loaders_keep_small_geometry_and_theme_contrast_in_every_context() {
        for theme in [Theme::dark(), Theme::light()] {
            for body_size in [14., 16., 24., 32.] {
                for slot_size in [None, Some(20.), Some(32.), Some(48.)] {
                    let ctx = Context::default();
                    theme.apply(&ctx);
                    let mut center = Pos2::ZERO;
                    let output = ctx.run(
                        RawInput {
                            time: Some(1.),
                            ..Default::default()
                        },
                        |ctx| {
                            CentralPanel::default().show(ctx, |ui| {
                                ui.style_mut().override_font_id =
                                    Some(FontId::proportional(body_size));
                                ui.spacing_mut().interact_size.y = body_size * 2.;
                                ui.visuals_mut().widgets.noninteractive.fg_stroke.color =
                                    if theme.dark {
                                        Color32::BLACK
                                    } else {
                                        Color32::WHITE
                                    };
                                if let Some(size) = slot_size {
                                    let slot =
                                        Rect::from_min_size(pos2(20., 20.), Vec2::splat(size));
                                    center = slot.center();
                                    ui.put(slot, spinner(&theme));
                                } else {
                                    let response = ui.add(spinner(&theme));
                                    assert_eq!(response.rect.size(), Vec2::splat(SIZE));
                                    center = response.rect.center();
                                }
                            });
                        },
                    );
                    let path = output
                        .shapes
                        .iter()
                        .find_map(|s| match &s.shape {
                            Shape::Path(p) => Some(p),
                            _ => None,
                        })
                        .expect("loader painted");
                    assert_eq!(
                        path.stroke.color,
                        egui::epaint::ColorMode::Solid(if theme.dark {
                            Color32::WHITE
                        } else {
                            color(theme.text)
                        })
                    );
                    assert_eq!(path.stroke.width, 3.);
                    for point in &path.points {
                        assert!(
                            (point.distance(center) - (SIZE / 2. - 2.)).abs() < 0.01,
                            "loader grew in a {slot_size:?} slot or {body_size}-point layout"
                        );
                    }
                }
            }
        }
    }
}
