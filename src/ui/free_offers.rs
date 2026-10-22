use super::*;
use engine_workspace::{
    fab,
    fab_offers::{self, Catalogue, Offer},
};

enum Event {
    Catalogue(Result<Catalogue, String>),
    Image(String, ColorImage),
}

#[derive(Default)]
pub(super) struct FreeOffers {
    catalogue: Option<Catalogue>,
    error: Option<String>,
    rx: Option<Receiver<Event>>,
    next_check: Option<Instant>,
    attempted: bool,
}

impl Workspace {
    pub(super) fn preview_free_offers(&mut self, ctx: &Context) {
        self.demo = true;
        if let Some(mut catalogue) = fab_offers::load_cache() {
            for offer in &catalogue.offers {
                if let Some(url) = &offer.thumbnail {
                    if fab::thumbnail_is_cached(url) {
                        if let Ok(image) = fab::thumbnail_image(url) {
                            let image = image.thumbnail(640, 360).to_rgba8();
                            self.textures.insert(
                                format!("free-offer:{}", offer.id),
                                ctx.load_texture(
                                    format!("free-offer:{}", offer.id),
                                    ColorImage::from_rgba_unmultiplied(
                                        [image.width() as usize, image.height() as usize],
                                        image.as_raw(),
                                    ),
                                    TextureOptions::LINEAR,
                                ),
                            );
                        }
                    }
                }
            }
            if std::env::args().any(|a| a == "--expired-offers-preview") {
                for offer in &mut catalogue.offers {
                    offer.ends_at = Some(time::OffsetDateTime::now_utc().unix_timestamp() - 60);
                }
                self.free_offers.error = Some("Fab did not respond. Try again later.".into());
            }
            self.free_offers.catalogue = Some(catalogue);
        }
    }
    pub(super) fn free_offer_countdown(&self) -> Option<String> {
        if self.prefs.hide_free_offer_countdown || self.free_offers.error.is_some() {
            return None;
        }
        let catalogue = self.free_offers.catalogue.as_ref()?;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        if catalogue.needs_refresh(now) {
            return None;
        }
        catalogue
            .ends_at()
            .map(|end| fab_offers::countdown(end, now))
    }

    fn refresh_free_offers(&mut self, ctx: &Context, initial: bool) {
        if self.free_offers.rx.is_some() {
            return;
        }
        self.free_offers.attempted = true;
        let (tx, rx) = mpsc::channel();
        self.free_offers.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let cached = initial.then(fab_offers::load_cache).flatten();
            if let Some(cached) = &cached {
                let _ = tx.send(Event::Catalogue(Ok(cached.clone())));
                ctx.request_repaint();
            }
            let catalogue = if cached
                .as_ref()
                .is_some_and(|c| !c.needs_refresh(time::OffsetDateTime::now_utc().unix_timestamp()))
            {
                cached
            } else {
                let result = fab_offers::fetch();
                let catalogue = result.as_ref().ok().cloned().or(cached);
                let _ = tx.send(Event::Catalogue(result));
                ctx.request_repaint();
                catalogue
            };
            if let Some(catalogue) = catalogue {
                for offer in catalogue.offers {
                    if let Some(url) = offer.thumbnail {
                        if let Ok(image) = fab::thumbnail_image(&url) {
                            let image = image.thumbnail(640, 360).to_rgba8();
                            let image = ColorImage::from_rgba_unmultiplied(
                                [image.width() as usize, image.height() as usize],
                                image.as_raw(),
                            );
                            let _ = tx.send(Event::Image(offer.id, image));
                            ctx.request_repaint();
                        }
                    }
                }
            }
        });
    }

    pub(super) fn poll_free_offers(&mut self, ctx: &Context) {
        loop {
            match self.free_offers.rx.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(Event::Catalogue(result))) => match result {
                    Ok(catalogue) => {
                        self.free_offers.catalogue = Some(catalogue);
                        self.free_offers.error = None;
                    }
                    Err(error) => self.free_offers.error = Some(error),
                },
                Some(Ok(Event::Image(id, image))) => {
                    self.textures.insert(
                        format!("free-offer:{id}"),
                        ctx.load_texture(format!("free-offer:{id}"), image, TextureOptions::LINEAR),
                    );
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.free_offers.rx = None;
                    if self.free_offers.catalogue.is_none() && self.free_offers.error.is_none() {
                        self.free_offers.error =
                            Some("The offer check stopped. Please refresh to try again.".into());
                    }
                    self.free_offers.next_check = Some(
                        Instant::now()
                            + Duration::from_secs(if self.free_offers.error.is_some() {
                                300
                            } else {
                                1
                            }),
                    );
                    break;
                }
                _ => break,
            }
        }
        if self.demo || self.layout_probe.is_some() {
            return;
        }
        let due = self
            .free_offers
            .next_check
            .is_none_or(|next| Instant::now() >= next)
            && (self.free_offers.error.is_some()
                || self.free_offers.catalogue.as_ref().is_none_or(|c| {
                    c.needs_refresh(time::OffsetDateTime::now_utc().unix_timestamp())
                }));
        if !self.free_offers.attempted || due {
            self.refresh_free_offers(ctx, !self.free_offers.attempted);
        }
        if self.page == Page::FreeOffers || !self.prefs.hide_free_offer_countdown {
            ctx.request_repaint_after(Duration::from_secs(30));
        }
    }

    pub(super) fn free_offers_page(&mut self, ui: &mut Ui, c: &Canvas) {
        let theme = self.theme.clone();
        let checking = self.free_offers.rx.is_some()
            || (self.layout_probe.is_some() && std::env::args().any(|a| a == "--loading-preview"));
        let top = PageHeaderLayout::new().divider_y + 18.;
        let rect = c.rect([271., top, c.width - 297., (c.height - top - 55.).max(160.)]);
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.style_mut().override_font_id = Some(FontId::proportional(16.));
            ui.spacing_mut().item_spacing = vec2(16., 12.);
            ui.spacing_mut().button_padding = vec2(14., 8.);
            AppScrollArea::vertical(&theme).id_salt("free-offers").show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!checking, buttons::secondary_button("Refresh now")).clicked() {
                        self.refresh_free_offers(ui.ctx(), false);
                    }
                    if ui.add(buttons::secondary_button(if self.prefs.hide_free_offer_countdown { "Show countdown" } else { "Hide countdown" })).clicked() {
                        self.prefs.hide_free_offer_countdown = !self.prefs.hide_free_offer_countdown;
                        self.persist();
                    }
                    if ui.add(buttons::secondary_button("Open offers on Fab ↗")).clicked() {
                        if let Err(error) = platform::open_external(fab_offers::PAGE_URL) { self.notify(error); }
                    }
                    if checking { ui.add(loading::spinner(&theme)); ui.label("Checking Fab…"); }
                });
                let now = time::OffsetDateTime::now_utc().unix_timestamp();
                let catalogue = self.free_offers.catalogue.clone();
                let stale = catalogue.as_ref().is_some_and(|c| c.needs_refresh(now)) || self.free_offers.error.is_some();
                highlighted_panel(&theme).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label("Claim these offers on Fab before they end. Offers refresh automatically; use Refresh now to check again.");
                    if let Some(error) = &self.free_offers.error {
                        ui.colored_label(color(theme.warning), format!("Could not refresh offers: {error}"));
                        ui.label("Saved offers may have changed. Open Fab to confirm availability.");
                    }
                    if let Some(catalogue) = &catalogue {
                        if let Some(end) = catalogue.ends_at() {
                            let label = fab_offers::end_label(end);
                            ui.label(RichText::new(if stale { label.replace("Next offer ends", "Saved offer end:") } else { label }).size(20.).strong().color(color(theme.text)));
                        }
                        if stale && self.free_offers.error.is_none() {
                            ui.colored_label(color(theme.warning), "Checking whether these saved offers are still available…");
                        }
                    } else if self.free_offers.rx.is_none() {
                        ui.label("Offer information is unavailable. You can still view the current offers on Fab.");
                    }
                });
                let Some(catalogue) = catalogue else { return; };
                if catalogue.offers.is_empty() { ui.label("Fab is not listing any limited-time free offers right now."); }
                let columns = ((ui.available_width() + 16.) / 316.).floor().clamp(1., 3.) as usize;
                let width = ((ui.available_width() - 16. * (columns - 1) as f32) / columns as f32).min(420.);
                for row in catalogue.offers.chunks(columns) {
                    ui.horizontal_top(|ui| {
                        for offer in row { self.free_offer_card(ui, &theme, offer, width, now, stale); }
                    });
                }
            });
        });
    }

    fn free_offer_card(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        offer: &Offer,
        width: f32,
        now: i64,
        stale: bool,
    ) {
        ui.push_id(&offer.id, |ui| {
            ui.allocate_ui_with_layout(vec2(width, 0.), Layout::top_down(Align::Min), |ui| {
                Frame::new()
                    .fill(color(theme.card))
                    .stroke(Stroke::new(1., color(theme.border)))
                    .corner_radius(6.)
                    .show(ui, |ui| {
                        ui.set_width(width - 2.);
                        let (picture, _) = ui.allocate_exact_size(
                            vec2(width - 2., (width - 2.) * 0.6),
                            Sense::hover(),
                        );
                        if let Some(texture) =
                            self.textures.get(&format!("free-offer:{}", offer.id))
                        {
                            paint_thumbnail(
                                ui,
                                texture,
                                picture,
                                Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                                thumbnail_top_corners(5.),
                            );
                        } else {
                            ui.painter()
                                .rect_filled(picture, 5., color(theme.selection));
                            downloads::draw_icon(
                                ui,
                                theme,
                                Icon::Layers,
                                Rect::from_center_size(picture.center(), vec2(42., 42.)),
                                theme.muted,
                            );
                        }
                        let expired = offer.ends_at.is_some_and(|end| now >= end);
                        let label = if expired {
                            "Ended"
                        } else if stale {
                            "Check on Fab"
                        } else {
                            "Free"
                        };
                        let badge_width = if label == "Check on Fab" { 116. } else { 58. };
                        let badge =
                            Rect::from_min_size(picture.min + vec2(8., 8.), vec2(badge_width, 26.));
                        ui.painter().rect_filled(
                            badge,
                            12.,
                            color(if stale || expired {
                                theme.card
                            } else {
                                theme.success
                            }),
                        );
                        ui.painter().text(
                            badge.center(),
                            Align2::CENTER_CENTER,
                            label,
                            FontId::proportional(13.),
                            if stale || expired {
                                color(theme.text)
                            } else if !theme.dark {
                                Color32::WHITE
                            } else {
                                Color32::BLACK
                            },
                        );
                        Frame::new().inner_margin(10).show(ui, |ui| {
                            ui.set_width(width - 22.);
                            ui.add(
                                Label::new(
                                    RichText::new(&offer.title)
                                        .strong()
                                        .color(color(theme.text)),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&offer.title);
                            ui.add(
                                Label::new(
                                    RichText::new(&offer.publisher).color(color(theme.muted)),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&offer.publisher);
                            let owned =
                                self.session.is_some() && self.library.owns_listing(&offer.id);
                            ui.label(
                                RichText::new(if owned {
                                    "Saved in My Library"
                                } else if self.session.is_none() {
                                    "Sign in to check your library"
                                } else {
                                    "Check ownership on Fab"
                                })
                                .color(color(if owned {
                                    theme.success
                                } else {
                                    theme.muted
                                })),
                            );
                            if ui.add(primary_button("Open on Fab", theme)).clicked() {
                                if let Err(error) = platform::open_external(&offer.url()) {
                                    self.notify(error);
                                }
                            }
                            if !self.prefs.hide_free_offer_countdown {
                                ui.label(if stale && !expired {
                                    "Availability needs checking".into()
                                } else {
                                    offer
                                        .ends_at
                                        .map(|end| fab_offers::countdown(end, now))
                                        .unwrap_or_else(|| "See Fab for offer end time".into())
                                });
                            }
                        });
                    });
            });
        });
    }
}
