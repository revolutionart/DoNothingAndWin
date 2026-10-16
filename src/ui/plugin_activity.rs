use super::*;
use engine_workspace::plugins::UpdateProgress;
use std::collections::VecDeque;

const INTERVAL: f64 = 0.5;
const SAMPLES: usize = 120;

pub(super) struct Card<'a> {
    pub title: &'a str,
    pub icon_key: &'a str,
    pub fallback_icon: &'a str,
    pub detail: String,
    pub fraction: Option<f32>,
    pub package_bytes: Option<u64>,
    pub downloaded: u64,
    pub files_done: usize,
    pub files_total: usize,
}

impl<'a> Card<'a> {
    pub fn engine(title: &'a str, p: &engine_workspace::installer::Progress) -> Self {
        let downloading = p.phase == "Downloading and verifying chunks";
        let preparing = p.phase == "Reconstructing and verifying engine files";
        let measured = (downloading || preparing) && p.total > 0;
        Self {
            title,
            icon_key: "",
            fallback_icon: "UE",
            detail: if measured {
                format!(
                    "{}  •  {} / {} {}",
                    p.phase,
                    bytes(p.completed),
                    bytes(p.total),
                    if downloading {
                        "download data ready"
                    } else {
                        "prepared"
                    }
                )
            } else if p.phase.is_empty() {
                "Preparing installation...".into()
            } else {
                p.phase.clone()
            },
            fraction: measured.then(|| (p.completed as f32 / p.total as f32).clamp(0., 1.)),
            package_bytes: (p.package_bytes > 0).then_some(p.package_bytes),
            downloaded: p.network,
            files_done: p.files,
            files_total: p.files_total,
        }
    }
}

#[derive(Default)]
pub(super) struct Activity {
    key: String,
    last: Option<(Instant, [u64; 3])>,
    samples: VecDeque<[f32; 3]>,
}
impl Activity {
    pub fn sample(&mut self, key: &str, state: &UpdateProgress, now: Instant) {
        self.sample_totals(key, [state.downloaded, state.read, state.written], now);
    }
    pub fn sample_totals(&mut self, key: &str, totals: [u64; 3], now: Instant) {
        if self.key != key
            || self
                .last
                .is_some_and(|(_, old)| totals.iter().zip(old).any(|(a, b)| *a < b))
        {
            *self = Self::default();
            self.key = key.into();
        }
        let Some((last, previous)) = self.last else {
            self.last = Some((now, totals));
            return;
        };
        let elapsed = now.duration_since(last).as_secs_f64();
        if elapsed < INTERVAL {
            return;
        }
        self.samples.push_back(std::array::from_fn(|i| {
            ((totals[i].saturating_sub(previous[i])) as f64 * 8. / elapsed / 1_000_000.) as f32
        }));
        while self.samples.len() > SAMPLES {
            self.samples.pop_front();
        }
        self.last = Some((now, totals));
    }

    pub fn show(
        &self,
        ui: &mut Ui,
        t: &Theme,
        p: &plugin_jobs::PluginProgress,
        textures: &HashMap<String, TextureHandle>,
        bordered: bool,
    ) {
        self.show_card(
            ui,
            t,
            &Card {
                title: &p.title,
                icon_key: &p.icon_key,
                fallback_icon: "P",
                detail: if p.state.total > 0 {
                    format!(
                        "{}  •  {} / {} prepared",
                        p.state.phase,
                        bytes(p.state.prepared),
                        bytes(p.state.total)
                    )
                } else {
                    p.state.phase.into()
                },
                fraction: p.state.fraction(),
                package_bytes: p.state.package_bytes,
                downloaded: p.state.downloaded,
                files_done: p.state.files_done,
                files_total: p.state.files_total,
            },
            textures,
            bordered,
        );
    }

    pub fn show_card(
        &self,
        ui: &mut Ui,
        t: &Theme,
        p: &Card<'_>,
        textures: &HashMap<String, TextureHandle>,
        bordered: bool,
    ) {
        Frame::new().fill(color(t.card)).stroke(if bordered { Stroke::new(1., color(t.border)) } else { Stroke::NONE })
            .corner_radius(10.).inner_margin(16).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(10., 5.);
                ui.spacing_mut().interact_size.y = 20.;
                ui.style_mut().override_font_id = Some(FontId::proportional(14.));
                let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), 66.), Sense::hover());
                let icon = Rect::from_min_size(header.min, vec2(48., 58.));
                ui.painter().rect_filled(icon, 6., color(t.background));
                if let Some(texture) = textures.get(p.icon_key) {
                    ui.painter().image(texture.id(), icon.shrink2(vec2(4., 9.)), Rect::from_min_max(Pos2::ZERO, pos2(1.,1.)), Color32::WHITE);
                } else {
                    ui.painter().text(icon.center(), Align2::CENTER_CENTER, p.fallback_icon, FontId::proportional(24.), color(t.muted));
                }
                let left = header.left() + 62.;
                let title_rect = Rect::from_min_max(pos2(left, header.top()), pos2(header.right(), header.top()+23.));
                ui.scope_builder(UiBuilder::new().max_rect(title_rect).layout(Layout::left_to_right(Align::Center)), |ui| {
                    ui.add(Label::new(RichText::new(p.title).strong().size(16.).color(color(t.text))).truncate()).on_hover_text(p.title);
                });
                let bar = Rect::from_min_max(pos2(left, header.top()+28.), pos2(header.right(), header.top()+33.));
                if let Some(fraction) = p.fraction {
                    ui.painter().rect_filled(bar, 2., color(t.border));
                    let filled = Rect::from_min_size(bar.min, vec2(bar.width()*fraction, bar.height()));
                    ui.painter().rect_filled(filled, 2., Color32::from_rgb(53, 187, 233));
                } else {
                    let mut bar_ui = ui.new_child(UiBuilder::new().max_rect(bar));
                    indeterminate_progress(&mut bar_ui, bar.width(), 5., t.accent, t.border);
                }
                let detail = &p.detail;
                let detail_rect = Rect::from_min_max(pos2(left, header.top()+39.), pos2(header.right(), header.bottom()));
                ui.scope_builder(UiBuilder::new().max_rect(detail_rect).layout(Layout::left_to_right(Align::Center)), |ui| {
                    ui.add(Label::new(RichText::new(detail).size(14.).color(color(t.muted))).truncate()).on_hover_text(detail);
                });
                ui.horizontal_wrapped(|ui| {
                    if let Some(size) = p.package_bytes {
                        ui.label(RichText::new(format!("Compressed package {}", bytes(size))).color(color(t.muted)))
                            .on_hover_text("Total compressed package size. Existing verified chunks can reduce the amount downloaded.");
                    }
                    ui.label(RichText::new(format!("Downloaded {}", bytes(p.downloaded))).color(color(t.muted)));
                    if p.files_total > 0 {
                        ui.label(RichText::new(format!("{} / {} files verified", p.files_done, p.files_total)).color(color(t.muted)));
                    }
                });
                ui.separator();
                ui.columns(3, |columns| {
                    for (i, column) in columns.iter_mut().enumerate() {
                        self.graph(column, t, i);
                    }
                });
            });
        ui.ctx().request_repaint_after(Duration::from_millis(150));
    }

    fn graph(&self, ui: &mut Ui, t: &Theme, index: usize) {
        let (r, response) = ui.allocate_exact_size(vec2(ui.available_width(), 76.), Sense::hover());
        let label = ["Download", "Read", "Write"][index];
        let colors = [
            Color32::from_rgb(112, 107, 224),
            Color32::from_rgb(173, 94, 215),
            Color32::from_rgb(220, 85, 163),
        ];
        let accent = colors[index];
        let rate = self.samples.back().map(|p| p[index]).unwrap_or(0.);
        let plot = Rect::from_min_max(
            r.min + vec2((r.width() * 0.34).min(108.), 6.),
            r.max - vec2(4., 19.),
        );
        let max = self.samples.iter().map(|s| s[index]).fold(1., f32::max) * 1.15;
        let step = plot.width() / (SAMPLES - 1) as f32;
        let mut points = vec![plot.left_bottom()];
        if !self.samples.is_empty() {
            points.push(pos2(
                plot.right() - (self.samples.len() - 1) as f32 * step,
                plot.bottom(),
            ));
        }
        for (i, sample) in self.samples.iter().enumerate() {
            let x = plot.right() - (self.samples.len() - 1 - i) as f32 * step;
            points.push(pos2(x, plot.bottom() - sample[index] / max * plot.height()));
        }
        if self.samples.is_empty() {
            points.push(plot.right_bottom());
        }
        let mut mesh = egui::Mesh::default();
        for pair in points.windows(2) {
            let base = mesh.vertices.len() as u32;
            for (point, alpha) in [
                (pair[0], 0.25),
                (pair[1], 0.25),
                (pos2(pair[1].x, plot.bottom()), 0.02),
                (pos2(pair[0].x, plot.bottom()), 0.02),
            ] {
                mesh.colored_vertex(point, accent.gamma_multiply(alpha));
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        ui.painter().add(Shape::mesh(mesh));
        ui.painter()
            .add(Shape::line(points, Stroke::new(1.5, accent)));
        ui.painter().text(
            r.min + vec2(4., 16.),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.),
            color(t.text),
        );
        ui.painter().text(
            r.left_bottom() + vec2(4., -8.),
            Align2::LEFT_BOTTOM,
            format!("{rate:.2} Mbps"),
            FontId::proportional(14.),
            color(t.muted),
        );
        if index > 0 {
            ui.painter().line_segment(
                [r.left_top(), r.left_bottom()],
                Stroke::new(1., color(t.border).gamma_multiply(0.5)),
            );
        }
        response.on_hover_text(match index {
            0 => "Received network bytes, including retries. Megabits per second.",
            1 => "Package bytes read from local storage, including verification reads. Megabits per second.",
            _ => "Installation and cache bytes written by the launcher. Megabits per second. Operating-system buffering can differ from physical disk activity.",
        });
    }
}
fn bytes(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.2} GB", n as f64 / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.2} MB", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1} KB", n as f64 / 1e3)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_phases_keep_network_separate_from_prepared_bytes() {
        let mut p = engine_workspace::installer::Progress {
            phase: "Downloading and verifying chunks".into(),
            completed: 500,
            total: 1000,
            network: 200,
            read: 800,
            written: 400,
            ..Default::default()
        };
        let card = Card::engine("UE", &p);
        assert_eq!(card.fraction, Some(0.5));
        assert_eq!(card.downloaded, 200);
        assert!(card.detail.ends_with("download data ready"));
        p.phase = "Reconstructing and verifying engine files".into();
        assert!(Card::engine("UE", &p).detail.ends_with("prepared"));
        p.phase = "Registering engine with Epic Games Launcher".into();
        assert_eq!(Card::engine("UE", &p).fraction, None);
        assert!(!Card::engine("UE", &p).detail.contains("prepared"));
    }
    #[test]
    fn rates_use_real_deltas_decay_when_idle_and_reset_for_next_plugin() {
        let mut a = Activity::default();
        let now = Instant::now();
        let mut p = UpdateProgress::default();
        a.sample("one", &p, now);
        p.downloaded = 1_000_000;
        p.read = 2_000_000;
        p.written = 3_000_000;
        a.sample("one", &p, now + Duration::from_secs(1));
        assert_eq!(a.samples.back(), Some(&[8., 16., 24.]));
        a.sample("one", &p, now + Duration::from_secs(2));
        assert_eq!(a.samples.back(), Some(&[0., 0., 0.]));
        a.sample(
            "two",
            &UpdateProgress::default(),
            now + Duration::from_secs(3),
        );
        assert!(a.samples.is_empty());
        for n in 4..200 {
            a.sample("two", &p, now + Duration::from_secs(n));
        }
        assert_eq!(a.samples.len(), SAMPLES);
    }
}
