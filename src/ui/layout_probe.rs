use super::*;

// Opt-in render regression probe. Uses the real native window and renderer;
// never starts transfers or writes workspace preferences.
pub(super) struct LayoutProbe {
    directory: std::path::PathBuf,
    step: usize,
    phase: u8,
    deadline: Instant,
}
const CASES: &[(f32, f32, bool)] = &[
    (1000., 680., false),
    (1200., 800., false),
    (1400., 900., false),
    (1586., 992., false),
    (1920., 1080., false),
    (1000., 680., true),
    (1200., 800., true),
    (1400., 900., true),
    (1586., 992., true),
    (1920., 1080., true),
];
impl LayoutProbe {
    pub(super) fn from_args() -> Option<Self> {
        let directory = std::env::args().find_map(|a| {
            a.strip_prefix("--layout-probe=")
                .map(std::path::PathBuf::from)
        })?;
        std::fs::create_dir_all(&directory).expect("create layout probe directory");
        Some(Self {
            directory,
            step: 0,
            phase: 0,
            deadline: Instant::now(),
        })
    }
}
impl Workspace {
    pub(super) fn run_layout_probe(&mut self, ctx: &Context) {
        let Some(mut probe) = self.layout_probe.take() else {
            return;
        };
        if probe.step >= CASES.len() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        let (width, height, plugins) = CASES[probe.step];
        if probe.phase == 0 {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(30., 30.)));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(width, height)));
            self.plugin_engine = None;
            self.project_scroll = 0.;
            if plugins && !self.engines.is_empty() {
                self.show_plugins((self.engines.len() - 1).min(1));
            }
            probe.phase = 1;
            probe.deadline = Instant::now() + Duration::from_millis(700);
        } else if probe.phase == 1 && Instant::now() >= probe.deadline {
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
            probe.phase = 2;
        } else if probe.phase == 2 {
            let screenshot = ctx.input(|i| {
                i.events.iter().find_map(|e| {
                    if let Event::Screenshot { image, .. } = e {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(image) = screenshot {
                let name = format!(
                    "{}-{}x{}",
                    if plugins { "plugins" } else { "workspace" },
                    width,
                    height
                );
                let pixels: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                image::save_buffer(
                    probe.directory.join(format!("{name}.png")),
                    &pixels,
                    image.width() as u32,
                    image.height() as u32,
                    image::ColorType::Rgba8,
                )
                .expect("save probe screenshot");
                std::fs::write(
                    probe.directory.join(format!("{name}.txt")),
                    format!(
                        "Requested: {width}x{height}\nActual logical client: {:?}\n",
                        ctx.content_rect().size()
                    ),
                )
                .expect("save probe dimensions");
                probe.step += 1;
                probe.phase = 0;
            }
        }
        self.layout_probe = Some(probe);
        ctx.request_repaint_after(Duration::from_millis(40));
    }
}
