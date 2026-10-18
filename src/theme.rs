use eframe::egui::Color32;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub background: [u8; 3],
    pub sidebar: [u8; 3],
    pub panel: [u8; 3],
    pub card: [u8; 3],
    pub border: [u8; 3],
    pub text: [u8; 3],
    pub muted: [u8; 3],
    pub subtle: [u8; 3],
    pub accent: [u8; 3],
    pub accent_hover: [u8; 3],
    pub selection: [u8; 3],
    pub success: [u8; 3],
    pub warning: [u8; 3],
    #[serde(default = "danger")]
    pub danger: [u8; 3],
    #[serde(default = "launch_background")]
    pub launch_background: [u8; 3],
    #[serde(default = "launch_hover")]
    pub launch_hover: [u8; 3],
    #[serde(default = "launch_foreground")]
    pub launch_foreground: [u8; 3],
    pub button: [u8; 3],
    pub on_accent: [u8; 3],
    pub logo: [u8; 3],
    pub dark: bool,
}
pub fn color(v: [u8; 3]) -> Color32 {
    Color32::from_rgb(v[0], v[1], v[2])
}
impl Theme {
    pub fn apply(&self, ctx: &eframe::egui::Context) {
        // The app has an explicit appearance preference; OS theme events must not
        // switch native egui controls while our painted surfaces keep this theme.
        ctx.set_theme(if self.dark {
            eframe::egui::Theme::Dark
        } else {
            eframe::egui::Theme::Light
        });
        ctx.set_visuals(self.visuals());
        ctx.style_mut(|style| {
            style.interaction.tooltip_delay = 0.;
            style.interaction.show_tooltips_only_when_still = false;
        });
    }
    pub fn dark() -> Self {
        serde_json::from_str(include_str!("../themes/dark.json")).expect("bundled dark theme")
    }
    pub fn light() -> Self {
        serde_json::from_str(include_str!("../themes/light.json")).expect("bundled light theme")
    }
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
    pub fn visuals(&self) -> eframe::egui::Visuals {
        let mut v = if self.dark {
            eframe::egui::Visuals::dark()
        } else {
            eframe::egui::Visuals::light()
        };
        v.override_text_color = None;
        v.interact_cursor = Some(eframe::egui::CursorIcon::PointingHand);
        v.widgets.noninteractive.fg_stroke.color = color(self.text);
        v.panel_fill = color(self.panel);
        v.window_fill = color(self.panel);
        v.extreme_bg_color = color(self.card);
        v.selection.bg_fill = color(self.accent);
        v.selection.stroke.color = color(self.on_accent);
        v.widgets.inactive.bg_fill = color(self.card);
        v.widgets.inactive.weak_bg_fill = color(self.card);
        v.widgets.inactive.bg_stroke = eframe::egui::Stroke::new(1., color(self.border));
        v.widgets.hovered.weak_bg_fill = color([234, 189, 93]);
        v.widgets.active.weak_bg_fill = color([234, 189, 93]);
        for widget in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
        ] {
            widget.fg_stroke.color = color(self.text);
            widget.corner_radius = eframe::egui::CornerRadius::same(5);
            widget.expansion = 0.;
        }
        for widget in [&mut v.widgets.hovered, &mut v.widgets.active] {
            widget.bg_fill = color([234, 189, 93]);
            widget.fg_stroke.color = Color32::BLACK;
        }
        v.widgets.hovered.bg_stroke = eframe::egui::Stroke::NONE;
        v.widgets.active.bg_stroke = eframe::egui::Stroke::NONE;
        v
    }
}

fn launch_background() -> [u8; 3] {
    [234, 189, 93]
}
fn launch_hover() -> [u8; 3] {
    [242, 187, 19]
}
fn launch_foreground() -> [u8; 3] {
    [0, 0, 0]
}

fn danger() -> [u8; 3] {
    [235, 85, 85]
}
