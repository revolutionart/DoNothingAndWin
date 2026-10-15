use super::*;
impl Workspace {
    fn category_is_empty(&self, category: &str) -> bool {
        // Hidden projects still belong to their category.
        !self
            .prefs
            .project_category_assignments
            .values()
            .any(|value| value == category)
    }
    fn remove_empty_category(&mut self, category: &str) {
        if !self.category_is_empty(category) {
            return;
        }
        self.prefs
            .project_categories
            .retain(|value| value != category);
        if self.category_filter.as_deref() == Some(category) {
            self.category_filter = None;
            self.project_scroll = 0.;
        }
        self.persist();
    }
    pub(super) fn project_category_menu(&mut self, ui: &mut Ui, path: &str) {
        ui.menu_button("Categories", |ui| {
            style_menu(ui, &self.theme);
            let current = self
                .prefs
                .project_category_assignments
                .get(path)
                .cloned()
                .unwrap_or_default();
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
            if !self.prefs.project_categories.is_empty() {
                ui.separator();
            }
            if ui
                .button("Add")
                .clone()
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked()
            {
                self.category_name.clear();
                self.category_add_project = Some(path.to_owned());
                ui.close();
            }
            if !current.is_empty()
                && ui
                    .button("Remove from category")
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
            {
                self.prefs.project_category_assignments.remove(path);
                self.persist();
                ui.close();
            }
        });
    }
    fn valid_category_name(&self, name: &str) -> bool {
        !name.is_empty()
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
                .any(|s| s.eq_ignore_ascii_case(name))
    }
    fn add_project_category_dialog(&mut self, ctx: &Context, path: &str) {
        let mut open = true;
        let mut added = false;
        dialog::show(
            ctx,
            &self.theme.clone(),
            "add-project-category",
            "Add category",
            "Create a category and add this project to it.",
            &mut open,
            |ui| {
                let input = ui.add(
                    TextEdit::singleline(&mut self.category_name)
                        .hint_text("Category name")
                        .margin(vec2(12., 10.))
                        .char_limit(40)
                        .desired_width(f32::INFINITY),
                );
                let name = self.category_name.trim().to_owned();
                let valid = self.valid_category_name(&name);
                let enter = input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if dialog::footer(ui, &self.theme, "Add", valid, false) || (valid && enter) {
                    self.prefs.project_categories.push(name.clone());
                    self.prefs
                        .project_category_assignments
                        .insert(path.to_owned(), name);
                    self.persist();
                    added = true;
                }
            },
        );
        if !open || added {
            self.category_add_project = None;
            self.category_name.clear();
        }
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
                        AppScrollArea::horizontal(&self.theme)
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
                                            let tab = ui
                                                .selectable_label(
                                                    self.category_filter.as_ref() == Some(&key),
                                                    format!("{label} ({count})"),
                                                )
                                                .clone()
                                                .on_hover_cursor(CursorIcon::PointingHand);
                                            let empty = self.category_is_empty(&key);
                                            tab.context_menu(|ui| {
                                                style_menu(ui, &self.theme);
                                                if ui.add_enabled(empty, Button::new("Remove category"))
                                                    .on_disabled_hover_text("Move all projects out of this category first, including hidden projects.")
                                                    .on_hover_cursor(CursorIcon::PointingHand).clicked() {
                                                    self.remove_empty_category(&key);
                                                    ui.close();
                                                }
                                            });
                                            if tab.on_hover_text("Right-click to remove an empty category.").clicked()
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
        if let Some(path) = self.category_add_project.clone() {
            self.add_project_category_dialog(ctx, &path);
            return;
        }
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
                            .margin(vec2(12., 10.))
                            .char_limit(40)
                            .desired_width(330.),
                    );
                    let name = self.category_name.trim().to_owned();
                    let valid = self.valid_category_name(&name);
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
                ui.label(
                    "Empty categories can be removed. Hidden projects still count as assigned.",
                );
                AppScrollArea::vertical(&self.theme).max_height(300.).show(ui, |ui| {
                    for category in self.prefs.project_categories.clone() {
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(&category);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add_enabled(self.category_is_empty(&category), Button::new("Remove category"))
                                    .on_disabled_hover_text("Move all projects out of this category first, including hidden projects.")
                                    .clone()
                                    .on_hover_cursor(CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    self.remove_empty_category(&category);
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
