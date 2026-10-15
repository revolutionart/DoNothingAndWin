use super::*;
impl Workspace {
    pub(super) fn project_card(&mut self, ui: &mut Ui, c: &Canvas, index: usize, r: [f32; 4]) {
        let p = self.projects[index].clone();
        let [x, y, w, h] = r;
        let photo = h - 64.;
        let t = self.theme.clone();
        let selected = self.selected == Some(index);
        let response = ui.interact(c.rect(r), Id::new(("project-card", index)), Sense::click());
        let exists = self.demo || self.workspace_cache.projects.get(&p.path) == Some(&true);
        let missing = exists
            && project_engine(&p, &self.engines).is_none_or(|i| {
                !self.demo
                    && self.workspace_cache.engines.get(&self.engines[i].path) == Some(&false)
            });
        c.boxed(
            ui,
            r,
            t.card,
            if selected { t.accent_hover } else { t.border },
            6.,
            if selected { 2. } else { 1. },
        );
        let image = c.rect([x + 2., y + 2., w - 4., photo - 2.]);
        if let Some(tex) = p.artwork.as_ref().and_then(|a| self.textures.get(a)) {
            let source = tex.size_vec2();
            let aspect = image.width() / image.height();
            let sa = source.x / source.y;
            let (uw, uh) = if sa > aspect {
                (aspect / sa, 1.)
            } else {
                (1., sa / aspect)
            };
            let uv = Rect::from_min_max(
                pos2((1. - uw) / 2., (1. - uh) / 2.),
                pos2((1. + uw) / 2., (1. + uh) / 2.),
            );
            ui.painter().image(tex.id(), image, uv, Color32::WHITE);
        } else {
            c.fill(ui, [x + 2., y + 2., w - 4., photo - 2.], t.selection, 3.);
            c.icon(
                ui,
                Icon::Folder,
                x + w / 2. - 24.,
                y + photo / 2. - 24.,
                48.,
                t.muted,
            );
        }
        if response
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            self.selected = Some(index);
        }
        if response.double_clicked() {
            self.open_project(index);
        }
        // Text is clipped to the footer, keeping project names out of the menu.
        let paint = ui
            .painter()
            .with_clip_rect(c.rect([x + 12., y + photo + 5., w - 54., 55.]));
        paint.text(
            c.point(x + 12., y + photo + 21.),
            Align2::LEFT_CENTER,
            &p.name,
            FontId::new(13. * c.scale(), FontFamily::Name("semibold".into())),
            color(t.text),
        );
        let age = if p.last_opened == 0 {
            "Last opened unknown".into()
        } else {
            let hours = epic::now().saturating_sub(p.last_opened) / 3600;
            if hours == 0 {
                "Opened just now".into()
            } else if hours < 24 {
                format!("Opened {hours} hours ago")
            } else if hours < 168 {
                format!("Opened {} days ago", hours / 24)
            } else {
                format!("Opened {} weeks ago", hours / 168)
            }
        };
        paint.text(
            c.point(x + 12., y + photo + 44.),
            Align2::LEFT_CENTER,
            age,
            FontId::proportional(11.5 * c.scale()),
            color(t.muted),
        );
        let label = EngineVersion::parse(&p.engine)
            .map(|v| v.branch())
            .unwrap_or_else(|| "Custom".into());
        let font = FontId::new(14. * c.scale(), FontFamily::Name("semibold".into()));
        let text_width = ui
            .painter()
            .layout_no_wrap(label.clone(), font, color(t.text))
            .size()
            .x
            / c.scale();
        let bw = 12. + text_width + if missing { 8. + 16. + 10. } else { 12. };
        let bx = x + w - bw - 10.;
        let by = y + photo - 40.;
        c.fill(ui, [bx, by, bw, 32.], t.card, 16.);
        c.text(
            ui,
            bx + 12.,
            by + 16.,
            &label,
            14.,
            true,
            t.text,
            Align2::LEFT_CENTER,
        );
        if missing {
            let cx = bx + bw - 18.;
            let cy = by + 16.;
            ui.painter().add(Shape::convex_polygon(
                vec![
                    c.point(cx, cy - 8.),
                    c.point(cx - 8., cy + 7.),
                    c.point(cx + 8., cy + 7.),
                ],
                color(t.warning),
                Stroke::NONE,
            ));
            c.text(
                ui,
                cx,
                cy + 1.,
                "!",
                12.,
                true,
                t.background,
                Align2::CENTER_CENTER,
            );
            ui.interact(c.rect([bx,by,bw,32.]),Id::new(("engine-warning",index)),Sense::hover()).on_hover_text(format!("Project exists, but its engine ({}) is not installed. Add or install that engine to open it.",p.engine));
        }
        // Child buttons take hover ownership from the card. Test its clipped
        // rectangle so entering the pin cannot make the pin disappear next frame.
        let card_hovered = ui.rect_contains_pointer(c.rect(r));
        if p.pinned || card_hovered || selected {
            if c.icon_button(
                ui,
                ("pin-project", index),
                [x + w - 39., y + 10., 28., 28.],
                Icon::Pin,
                p.pinned,
            ) {
                self.projects[index].pinned = !p.pinned;
                self.persist();
            }
        }
        let menu_rect = button_bounds([x + w - 43., y + photo + 11., 32., 38.]);
        let menu = ui
            .interact(
                c.rect(menu_rect),
                Id::new(("project-actions", index)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        c.boxed(
            ui,
            menu_rect,
            if menu.hovered() {
                SIDEBAR_HIGHLIGHT
            } else {
                t.card
            },
            t.border,
            4.,
            1.,
        );
        c.icon(
            ui,
            Icon::Dots,
            menu_rect[0] + 7. * BUTTON_SCALE,
            menu_rect[1] + 10. * BUTTON_SCALE,
            18. * BUTTON_SCALE,
            if menu.hovered() {
                SIDEBAR_ON_HIGHLIGHT
            } else {
                t.muted
            },
        );
        if response.secondary_clicked() {
            self.selected = Some(index);
        }
        Popup::menu(&menu)
            .frame(menu_frame(&self.theme))
            .show(|ui| self.project_options(ui, index));
        Popup::context_menu(&response)
            .frame(menu_frame(&self.theme))
            .show(|ui| self.project_options(ui, index));
    }

    pub(super) fn project_options(&mut self, ui: &mut Ui, index: usize) {
        style_menu(ui, &self.theme);
        let p = self.projects[index].clone();
        self.project_category_menu(ui, &p.path);
        self.project_visibility_menu(ui, &p.path);
        if ui
            .add_enabled(
                !self.demo && self.workspace_cache.projects.get(&p.path) == Some(&true),
                Button::new("Create desktop shortcut"),
            )
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            match platform::create_project_shortcut(&p) {
                Ok(path) => self.notify(format!("Desktop shortcut created: {}", path.display())),
                Err(e) => self.notify(e),
            }
            ui.close();
        }

        if ui
            .add_enabled(
                !self.demo && !self.cloning.busy(),
                Button::new("Clone project"),
            )
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            self.begin_clone(p.clone());
            ui.close();
        }
        if ui
            .button(if p.pinned { "Unpin" } else { "Pin project" })
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            self.projects[index].pinned = !p.pinned;
            self.persist();
            ui.close();
        }
        if ui
            .add_enabled(!self.demo, Button::new("Show in Folder"))
            .clone()
            .on_hover_cursor(CursorIcon::PointingHand)
            .clicked()
        {
            if let Some(folder) = Path::new(&p.path).parent() {
                if let Err(e) = platform::open_external(&folder.to_string_lossy()) {
                    self.notify(e);
                }
            }
            ui.close();
        }
    }
}
