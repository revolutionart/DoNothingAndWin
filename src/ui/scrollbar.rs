use super::*;
use egui::scroll_area::ScrollAreaOutput;

pub(super) const WIDTH: f32 = 10.;
pub(super) const MIN_THUMB: f32 = 40.;
pub(super) const MARGIN: f32 = 2.;
pub(super) const ALLOCATED_WIDTH: f32 = WIDTH + 2. * MARGIN;

/// One scrollbar treatment for pages, nested lists and dialogs. Content retains
/// its own visuals, so scrollbar hover colors never leak into buttons or labels.
pub(super) struct AppScrollArea {
    area: egui::ScrollArea,
    theme: Theme,
}
impl AppScrollArea {
    pub fn vertical(theme: &Theme) -> Self {
        Self {
            area: egui::ScrollArea::vertical(),
            theme: theme.clone(),
        }
    }
    pub fn horizontal(theme: &Theme) -> Self {
        Self {
            area: egui::ScrollArea::horizontal(),
            theme: theme.clone(),
        }
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
            self.area.show_rows(ui, height, count, |ui, range| {
                *ui.visuals_mut() = content;
                body(ui, range)
            })
        })
        .inner
    }
}
fn apply(ui: &mut Ui, theme: &Theme) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.floating = false;
    scroll.foreground_color = false;
    scroll.bar_width = WIDTH;
    scroll.handle_min_length = MIN_THUMB;
    scroll.bar_inner_margin = MARGIN;
    scroll.bar_outer_margin = MARGIN;
    let visuals = ui.visuals_mut();
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

pub(super) fn thumb_length(track: f32, visible: f32, content: f32) -> f32 {
    (track * visible / content.max(1.))
        .max(MIN_THUMB)
        .min(track)
}

#[cfg(test)]
mod tests {
    use super::*;
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
