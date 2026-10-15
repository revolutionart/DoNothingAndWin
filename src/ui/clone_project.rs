use super::*;
enum Event {
    Progress(usize, usize),
    Done(Result<Project, String>),
}
#[derive(Default)]
pub(super) struct Cloning {
    source: Option<Project>,
    name: String,
    destination: String,
    status: String,
    rx: Option<Receiver<Event>>,
}
impl Cloning {
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
}
impl Workspace {
    pub(super) fn begin_clone(&mut self, p: Project) {
        let name = format!("{}_Copy", p.name);
        let destination = Path::new(&p.path)
            .parent()
            .and_then(Path::parent)
            .unwrap_or(Path::new("C:/"))
            .join(&name)
            .to_string_lossy()
            .into_owned();
        self.cloning = Cloning {
            source: Some(p),
            name,
            destination,
            ..Default::default()
        };
    }
    pub(super) fn clone_dialog(&mut self, ctx: &Context) {
        while let Some(event) = self.cloning.rx.as_ref().and_then(|r| r.try_recv().ok()) {
            match event {
                Event::Progress(n, total) => {
                    self.cloning.status = format!("Copying project: {n} / {total} files")
                }
                Event::Done(result) => {
                    self.cloning.rx = None;
                    match result {
                        Ok(p) => {
                            self.prefs.manual_projects.push(p.path.clone());
                            self.projects.push(p);
                            self.load_assets(ctx);
                            self.persist();
                            self.cloning.source = None;
                            self.notify("Project cloned. Original project unchanged.");
                        }
                        Err(e) => self.cloning.status = e,
                    }
                }
            }
        }
        if self.cloning.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let Some(source) = self.cloning.source.clone() else {
            return;
        };
        let mut open = true;
        let mut start = false;
        dialog::show(
            ctx,
            &self.theme.clone(),
            "clone-project",
            "Clone project",
            "Choose a name and location for the project copy.",
            &mut open,
            |ui| {
                ui.add_enabled_ui(!self.cloning.busy(), |ui| {
                    ui.label("Project name");
                    let previous = self.cloning.name.clone();
                    if ui
                        .add_sized(
                            [ui.available_width(), 40.],
                            TextEdit::singleline(&mut self.cloning.name).margin(vec2(12., 8.)),
                        )
                        .changed()
                    {
                        let dest = Path::new(&self.cloning.destination);
                        if dest.file_name().is_some_and(|s| s == previous.as_str()) {
                            if let Some(parent) = dest.parent() {
                                self.cloning.destination = parent
                                    .join(&self.cloning.name)
                                    .to_string_lossy()
                                    .into_owned();
                            }
                        }
                    }
                    ui.label("Destination folder");
                    dialog::folder(ui, &mut self.cloning.destination);
                });
                dialog::info(
                    ui,
                    &self.theme,
                    "New project file",
                    &Path::new(&self.cloning.destination)
                        .join(format!("{}.uproject", self.cloning.name))
                        .to_string_lossy(),
                );
                if !self.cloning.status.is_empty() {
                    ui.label(&self.cloning.status);
                }
                if self.cloning.busy() {
                    ui.spinner();
                    ui.label("You can close this dialog; cloning will continue in the background.");
                }
                start = dialog::footer(
                    ui,
                    &self.theme,
                    "Clone project",
                    !self.demo
                        && !self.cloning.busy()
                        && !self.cloning.name.is_empty()
                        && !self.cloning.destination.is_empty(),
                    false,
                );
            },
        );
        if !open {
            self.cloning.source = None;
        }
        if start {
            let name = self.cloning.name.clone();
            let dest = self.cloning.destination.clone();
            let (tx, rx) = mpsc::channel();
            self.cloning.rx = Some(rx);
            self.cloning.status = "Preparing project copy...".into();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = engine_workspace::project_clone::clone_project(
                    &source,
                    &name,
                    Path::new(&dest),
                    |n, t| {
                        let _ = tx.send(Event::Progress(n, t));
                        ctx.request_repaint();
                    },
                );
                let _ = tx.send(Event::Done(result));
                ctx.request_repaint();
            });
        }
    }
}
