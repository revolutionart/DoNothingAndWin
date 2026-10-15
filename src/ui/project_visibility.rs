use super::*;

impl Workspace {
    // Visibility affects presentation only; discovery and saved projects remain intact.
    pub(super) fn project_visible(&self, path: &str) -> bool {
        !self.prefs.hidden_projects.contains(path)
    }
    pub(super) fn project_visibility_menu(&mut self, ui: &mut Ui, path: &str) {
        if ui
            .button("Hide project")
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(
                "Hide from project lists until restored in Settings. Project files are kept.",
            )
            .clicked()
        {
            self.prefs.hidden_projects.insert(path.to_owned());
            self.selected = None;
            self.persist();
            ui.close();
        }
    }
    pub(super) fn hidden_project_preferences(&mut self, ui: &mut Ui) {
        ui.add_space(12.);
        ui.label(
            RichText::new("Hidden projects")
                .strong()
                .color(color(self.theme.text)),
        );
        ui.label("Right-click a project and choose Hide project. Hidden projects stay hidden after restarting; their files remain untouched.");
        let count = self.prefs.hidden_projects.len();
        if ui
            .add_enabled(
                count > 0,
                Button::new(format!("Restore all hidden projects ({count})")),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            self.prefs.hidden_projects.clear();
            self.project_scroll = 0.;
            self.persist();
        }
        if count > 0 {
            egui::CollapsingHeader::new(
                RichText::new("Restore individual projects").color(color(self.theme.text)),
            )
            .show(ui, |ui| {
                let mut paths: Vec<_> = self.prefs.hidden_projects.iter().cloned().collect();
                paths.sort();
                for path in paths {
                    let name = self
                        .projects
                        .iter()
                        .find(|p| p.path == path)
                        .map(|p| p.name.as_str())
                        .unwrap_or(&path)
                        .to_owned();
                    ui.horizontal(|ui| {
                        ui.label(name).on_hover_text(&path);
                        if ui
                            .button("Restore")
                            .on_hover_cursor(CursorIcon::PointingHand)
                            .clicked()
                        {
                            self.prefs.hidden_projects.remove(&path);
                            self.project_scroll = 0.;
                            self.persist();
                        }
                    });
                }
            });
        }
    }
}
