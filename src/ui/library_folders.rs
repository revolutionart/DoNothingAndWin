use super::*;
use engine_workspace::library_organization::{FolderFilter, Organization};

#[derive(Default)]
pub(super) struct FolderView {
    pub filter: FolderFilter,
    pub show_hidden: bool,
    manager: bool,
    name: String,
    renaming: Option<String>,
    add_item: Option<String>,
    error: Option<String>,
}

impl Workspace {
    pub(super) fn library_organization(&self) -> Organization {
        self.session
            .as_ref()
            .and_then(|session| self.prefs.library_organization.get(&session.account_id))
            .cloned()
            .unwrap_or_default()
    }

    fn change_library_organization(
        &mut self,
        change: impl FnOnce(&mut Organization) -> Result<(), String>,
    ) -> Result<(), String> {
        let account = &self
            .session
            .as_ref()
            .ok_or("Connect your Epic account first.")?
            .account_id;
        change(
            self.prefs
                .library_organization
                .entry(account.clone())
                .or_default(),
        )?;
        self.persist();
        Ok(())
    }

    pub(super) fn library_folder_toolbar(&mut self, ui: &mut Ui, keys: &[String]) {
        let org = self.library_organization();
        let show_hidden = self.library.organization.show_hidden;
        let count = |filter: &FolderFilter| {
            keys.iter()
                .filter(|key| org.matches(key, filter, show_hidden))
                .count()
        };
        let selected = match &self.library.organization.filter {
            FolderFilter::All => "All items",
            FolderFilter::Unfiled => "Unfiled",
            FolderFilter::Folder(name) => name,
        }
        .to_owned();
        ui.horizontal_wrapped(|ui| {
            ui.label("Folder");
            AppComboBox::new(&self.theme, "library-folder-filter")
                .width(210.)
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.library.organization.filter, FolderFilter::All, format!("All items ({})", count(&FolderFilter::All)));
                    ui.selectable_value(&mut self.library.organization.filter, FolderFilter::Unfiled, format!("Unfiled ({})", count(&FolderFilter::Unfiled)));
                    for name in org.folders() {
                        let filter = FolderFilter::Folder(name.clone());
                        let label = format!("{name} ({})", count(&filter));
                        ui.selectable_value(&mut self.library.organization.filter, filter, label);
                    }
                });
            if ui.add(buttons::secondary_button("Manage folders")).clicked() {
                self.library.organization.manager = true;
                self.library.organization.add_item = None;
                self.library.organization.renaming = None;
                self.library.organization.name.clear();
                self.library.organization.error = None;
            }
            let hidden = keys.iter().filter(|key| org.is_hidden(key)).count();
            ui.add(buttons::checkbox(&mut self.library.organization.show_hidden, format!("Show hidden ({hidden})"), &self.theme))
                .on_hover_text("Show only hidden items matching the current filters. Use the crossed-out eye to restore an item to your normal Library view.");
        });
    }

    pub(super) fn library_item_menu(
        &mut self,
        ui: &mut Ui,
        picture: Rect,
        thumbnail: &Response,
        item: &engine_workspace::fab::Item,
        org: &Organization,
        show_controls: bool,
    ) {
        let key = item.key();
        let hidden = org.is_hidden(&key);
        Popup::context_menu(thumbnail)
            .frame(menu_frame(&self.theme))
            .show(|ui| {
                self.library_item_options(ui, &key, org);
            });
        if !show_controls {
            return;
        }
        let folder_rect = Rect::from_center_size(
            pos2(picture.right() - 20., picture.top() + 88.),
            Vec2::splat(28.),
        );
        let response = folder_button(ui, folder_rect, ui.id().with((&key, "folder")), &self.theme);
        Popup::menu(&response)
            .id(ui.id().with((&key, "folder-menu")))
            .frame(menu_frame(&self.theme))
            .show(|ui| {
                style_menu(ui, &self.theme);
                ui.label("Move to folder");
                ui.separator();
                self.library_folder_destinations(ui, &key, org);
            });
        let eye_rect = Rect::from_center_size(
            pos2(picture.right() - 20., picture.top() + 54.),
            Vec2::splat(28.),
        );
        let eye = visibility_button(
            ui,
            eye_rect,
            ui.id().with((&key, "visibility")),
            hidden,
            &self.theme,
        );
        if eye.clicked() {
            if let Err(error) = self.change_library_organization(|org| {
                org.set_hidden(&key, !hidden);
                Ok(())
            }) {
                self.notify(error);
            }
        }
    }

    fn library_item_options(&mut self, ui: &mut Ui, key: &str, org: &Organization) {
        style_menu(ui, &self.theme);
        ui.label(format!("Folder: {}", org.folder(key).unwrap_or("Unfiled")));
        ui.separator();
        ui.menu_button("Move to folder", |ui| {
            style_menu(ui, &self.theme);
            self.library_folder_destinations(ui, key, org);
        });
    }

    fn library_folder_destinations(&mut self, ui: &mut Ui, key: &str, org: &Organization) {
        let mut destination = None;
        if ui
            .selectable_label(org.folder(key).is_none(), "Unfiled")
            .clicked()
        {
            destination = Some(None);
        }
        for folder in org.folders() {
            if ui
                .selectable_label(org.folder(key) == Some(folder.as_str()), folder)
                .clicked()
            {
                destination = Some(Some(folder.clone()));
            }
        }
        if let Some(folder) = destination {
            if let Err(error) =
                self.change_library_organization(|org| org.move_item(key, folder.as_deref()))
            {
                self.notify(error);
            }
            ui.close();
        }
        ui.separator();
        if ui.add(buttons::secondary_button("New folder…")).clicked() {
            self.library.organization.manager = true;
            self.library.organization.add_item = Some(key.to_owned());
            self.library.organization.name.clear();
            self.library.organization.renaming = None;
            self.library.organization.error = None;
            ui.close();
        }
    }

    pub(super) fn library_folder_dialog(&mut self, ctx: &Context) {
        if !self.library.organization.manager {
            return;
        }
        let mut open = true;
        let theme = self.theme.clone();
        dialog::show(
            ctx,
            &theme,
            "library-folders",
            "Library folders",
            "Organize your Fab library without moving files. Folders are saved for this Epic account on this PC.",
            &mut open,
            |ui| {
                buttons::compact_style(ui);
                ui.add_space(8.);
                let renaming = self.library.organization.renaming.clone();
                ui.horizontal(|ui| {
                    let action_width = 126.;
                    let gap = ui.spacing().item_spacing.x;
                    let reserved = if renaming.is_some() {
                        2. * (action_width + gap)
                    } else {
                        action_width + gap
                    };
                    let input_width = (ui.available_width() - reserved).max(100.);
                    let input = ui.add(
                        TextEdit::singleline(&mut self.library.organization.name)
                            .hint_text(if renaming.is_some() {
                                "Rename folder"
                            } else {
                                "New folder name"
                            })
                            .char_limit(40)
                            .margin(vec2(10., 7.))
                            .desired_width(input_width - 20.),
                    );
                    let name = self.library.organization.name.trim().to_owned();
                    let valid = !name.is_empty();
                    let enter = input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if ui
                        .add_enabled(
                            valid,
                            buttons::primary_button(
                                if renaming.is_some() {
                                    "Save name"
                                } else {
                                    "Create folder"
                                },
                                &theme,
                            )
                            .min_size(vec2(action_width, COMPACT_BUTTON_HEIGHT)),
                        )
                        .clicked()
                        || (valid && enter)
                    {
                        let add_item = self.library.organization.add_item.clone();
                        let mut saved_name = String::new();
                        let result = self.change_library_organization(|org| {
                            saved_name = if let Some(old) = &renaming {
                                org.rename_folder(old, &name)?
                            } else {
                                org.create_folder(&name)?
                            };
                            if let Some(key) = &add_item {
                                org.move_item(key, Some(&saved_name))?;
                            }
                            Ok(())
                        });
                        match result {
                            Ok(()) => {
                                if renaming.as_ref().is_some_and(|old| {
                                    self.library.organization.filter
                                        == FolderFilter::Folder(old.clone())
                                }) || add_item.is_some()
                                {
                                    self.library.organization.filter =
                                        FolderFilter::Folder(saved_name);
                                }
                                self.library.organization.name.clear();
                                self.library.organization.renaming = None;
                                self.library.organization.add_item = None;
                                self.library.organization.error = None;
                            }
                            Err(error) => self.library.organization.error = Some(error),
                        }
                    }
                    if renaming.is_some()
                        && ui
                            .add(
                                buttons::secondary_button("Cancel rename")
                                    .min_size(vec2(action_width, COMPACT_BUTTON_HEIGHT)),
                            )
                            .clicked()
                    {
                        self.library.organization.renaming = None;
                        self.library.organization.name.clear();
                        self.library.organization.error = None;
                    }
                });
                if self.library.organization.add_item.is_some() {
                    ui.label("The selected item will be moved into the new folder.");
                }
                if let Some(error) = &self.library.organization.error {
                    ui.colored_label(color(theme.warning), error);
                }
                ui.add_space(4.);
                ui.label(RichText::new("Deleting a folder returns its items to Unfiled. Hidden items stay hidden; nothing is deleted from Fab or your PC.").color(color(theme.muted)));
                let org = self.library_organization();
                AppScrollArea::vertical(&theme).id_salt("library-folder-manager").max_height(280.).auto_shrink([false, true]).inner_margin(10.).show(ui, |ui| {
                if org.folders().is_empty() { ui.label("No folders yet. Create one above, then use an item's menu to move it."); }
                for folder in org.folders() {
                    ui.separator();
                    ui.horizontal(|ui| {
                        let actions_width = 126. + 90. + 2. * ui.spacing().item_spacing.x;
                        let (name_rect, name_response) = ui.allocate_exact_size(
                            vec2((ui.available_width() - actions_width).max(70.), COMPACT_BUTTON_HEIGHT), Sense::hover());
                        let galley = ui.painter().layout(folder.clone(), FontId::proportional(COMPACT_BUTTON_FONT_SIZE), color(theme.text), f32::INFINITY);
                        ui.painter().with_clip_rect(name_rect).galley(
                            pos2(name_rect.left(), name_rect.center().y - galley.size().y / 2.), galley, color(theme.text));
                        name_response.on_hover_text(folder);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(buttons::secondary_button("Delete folder").min_size(vec2(126., COMPACT_BUTTON_HEIGHT))).clicked() {
                                let result = self.change_library_organization(|org| { org.delete_folder(folder); Ok(()) });
                                if let Err(error) = result { self.library.organization.error = Some(error); }
                                else {
                                    if self.library.organization.filter == FolderFilter::Folder(folder.clone()) { self.library.organization.filter = FolderFilter::Unfiled; }
                                    if self.library.organization.renaming.as_ref() == Some(folder) { self.library.organization.renaming = None; self.library.organization.name.clear(); }
                                }
                            }
                            if ui.add(buttons::secondary_button("Rename").min_size(vec2(90., COMPACT_BUTTON_HEIGHT))).clicked() {
                                self.library.organization.renaming = Some(folder.clone());
                                self.library.organization.name = folder.clone();
                                self.library.organization.add_item = None;
                                self.library.organization.error = None;
                            }
                        });
                    });
                }
            });
            },
        );
        self.library.organization.manager = open;
    }
}

impl FolderView {
    pub(super) fn preview(&mut self, step: usize) {
        self.show_hidden = step >= 3;
        self.manager = step == 4;
    }
}

/// Visibility is a direct action; its icon shows the current state.
fn visibility_button(ui: &mut Ui, rect: Rect, id: Id, hidden: bool, theme: &Theme) -> Response {
    let label = if hidden {
        "Show in Library"
    } else {
        "Hide from Library"
    };
    let (response, fill, tint) = thumbnail_circle_button(ui, rect, id, label, theme);
    let painter = ui.painter();
    let center = rect.center();
    for direction in [-1., 1.] {
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [
                center + vec2(-9., 0.),
                center + vec2(-4., direction * 8.),
                center + vec2(4., direction * 8.),
                center + vec2(9., 0.),
            ],
            false,
            Color32::TRANSPARENT,
            Stroke::new(1.6, tint),
        ));
    }
    painter.circle_filled(center, 2.8, tint);
    if hidden {
        let slash = [center + vec2(-8., 9.), center + vec2(8., -9.)];
        painter.line_segment(slash, Stroke::new(4.5, fill));
        painter.line_segment(slash, Stroke::new(1.8, tint));
    }
    response.on_hover_text(label)
}

fn thumbnail_circle_button(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    label: &str,
    theme: &Theme,
) -> (Response, Color32, Color32) {
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    let highlighted =
        response.hovered() || response.has_focus() || response.is_pointer_button_down_on();
    let fill = color(if highlighted {
        theme.accent
    } else {
        theme.card
    });
    let tint = if highlighted {
        Color32::WHITE
    } else {
        color(theme.text)
    };
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 14., fill);
    (response, fill, tint)
}

fn folder_button(ui: &mut Ui, rect: Rect, id: Id, theme: &Theme) -> Response {
    let (response, _, tint) = thumbnail_circle_button(ui, rect, id, "Move to folder", theme);
    let center = rect.center();
    let points = [
        (-8., -6.),
        (-2., -6.),
        (1., -3.),
        (8., -3.),
        (8., 6.),
        (-8., 6.),
    ]
    .into_iter()
    .map(|(x, y)| center + vec2(x, y))
    .collect();
    ui.painter()
        .add(Shape::closed_line(points, Stroke::new(1.6, tint)));
    response.on_hover_text("Move to folder")
}
