use super::*;
impl Workspace {
    pub(super) fn project_category_menu(&mut self, ui: &mut Ui, path: &str) {
        ui.menu_button("Categories", |ui| {
            style_menu(ui, &self.theme);
            let current = self
                .prefs
                .project_category_assignments
                .get(path)
                .cloned()
                .unwrap_or_default();
            if ui
                .button("Remove from category")
                .clone()
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked()
            {
                self.prefs.project_category_assignments.remove(path);
                self.persist();
                ui.close();
            }
            for category in self.prefs.project_categories.clone() {
                if ui
                    .selectable_label(current == category, &category)
                    .clone()
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                {
                    self.prefs
                        .project_category_assignments
                        .insert(path.into(), category);
                    self.persist();
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .button("Manage categories…")
                .clone()
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked()
            {
                self.category_manager = true;
                ui.close();
            }
        });
    }
    pub(super) fn category_tabs(&mut self, ui: &mut Ui, c: &Canvas, y: f32) {
        let rect = c.rect([271., y, c.width - 303., 33.]);
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.horizontal(|ui| {
                let tab_width = ui.available_width().max(150.);
                ui.allocate_ui_with_layout(
                    vec2(tab_width, 31.),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ScrollArea::horizontal()
                            .id_salt("project-category-tabs")
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if ui
                                        .selectable_label(
                                            !self.pinned_only && self.category_filter.is_none(),
                                            "All projects",
                                        )
                                        .clone()
                                        .on_hover_cursor(CursorIcon::PointingHand)
                                        .clicked()
                                    {
                                        self.pinned_only = false;
                                        self.category_filter = None;
                                        self.project_scroll = 0.;
                                    }
                                    if ui
                                        .selectable_label(
                                            self.pinned_only,
                                            format!(
                                                "Pinned ({})",
                                                self.projects
                                                    .iter()
                                                    .filter(|p| p.pinned
                                                        && self.project_visible(&p.path))
                                                    .count()
                                            ),
                                        )
                                        .clone()
                                        .on_hover_cursor(CursorIcon::PointingHand)
                                        .clicked()
                                    {
                                        self.pinned_only = true;
                                        self.category_filter = None;
                                        self.project_scroll = 0.;
                                    }
                                    if !self.prefs.project_categories.is_empty() {
                                        let categories: Vec<_> = self
                                            .prefs
                                            .project_categories
                                            .iter()
                                            .map(|s| (s.clone(), s.clone()))
                                            .collect();
                                        for (key, label) in categories {
                                            let count = self
                                                .projects
                                                .iter()
                                                .filter(|p| self.project_visible(&p.path))
                                                .filter(|p| {
                                                    self.prefs
                                                        .project_category_assignments
                                                        .get(&p.path)
                                                        .map(String::as_str)
                                                        .unwrap_or("")
                                                        == key
                                                })
                                                .count();
                                            if ui
                                                .selectable_label(
                                                    self.category_filter.as_ref() == Some(&key),
                                                    format!("{label} ({count})"),
                                                )
                                                .clone()
                                                .on_hover_cursor(CursorIcon::PointingHand)
                                                .clicked()
                                            {
                                                self.pinned_only = false;
                                                self.category_filter = Some(key);
                                                self.project_scroll = 0.;
                                            }
                                        }
                                    }
                                });
                            });
                    },
                );
            });
        });
    }
    pub(super) fn category_dialog(&mut self, ctx: &Context) {
        if !self.category_manager {
            return;
        }
        let mut open = true;
        dialog::show(
            ctx,
            &self.theme.clone(),
            "project-categories",
            "Project categories",
            "Organize projects without moving their files.",
            &mut open,
            |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut self.category_name)
                            .hint_text("New category name")
                            .char_limit(40)
                            .desired_width(330.),
                    );
                    let name = self.category_name.trim().to_owned();
                    let valid = !name.is_empty()
                        && ![
                            "all projects",
                            "pinned",
                            "none",
                            "default",
                            "none / default",
                        ]
                        .contains(&name.to_lowercase().as_str())
                        && !self
                            .prefs
                            .project_categories
                            .iter()
                            .any(|s| s.eq_ignore_ascii_case(&name));
                    if ui
                        .add_enabled(valid, Button::new("Add category"))
                        .clone()
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.prefs.project_categories.push(name);
                        self.category_name.clear();
                        self.persist();
                    }
                });
                ui.label("Deleting a category keeps its projects under All projects. Project files stay untouched.");
                ScrollArea::vertical().max_height(300.).show(ui, |ui| {
                    for category in self.prefs.project_categories.clone() {
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(&category);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .button("Delete category")
                                    .clone()
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    self.prefs.project_categories.retain(|s| s != &category);
                                    self.prefs
                                        .project_category_assignments
                                        .retain(|_, v| v != &category);
                                    if self.category_filter.as_ref() == Some(&category) {
                                        self.category_filter = None;
                                    }
                                    self.persist();
                                }
                            });
                        });
                    }
                });
            },
        );
        self.category_manager = open;
    }
}
