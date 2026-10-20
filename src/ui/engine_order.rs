use super::*;

pub(super) struct EngineCardDrag {
    pub path: String,
}

fn key(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("//?/")
        .trim_end_matches('/')
        .to_lowercase()
}

// Order presentation indices only: active dialogs and actions retain their
// installation indices, and moving a card never changes the default engine.
pub(super) fn indices(engines: &[Installation], saved: &[String]) -> Vec<usize> {
    let ranks: HashMap<_, _> = saved
        .iter()
        .enumerate()
        .rev()
        .map(|(i, path)| (key(path), i))
        .collect();
    let mut order: Vec<_> = (0..engines.len()).collect();
    order.sort_by_key(|&i| {
        ranks
            .get(&key(&engines[i].path))
            .copied()
            .unwrap_or(usize::MAX)
    });
    order
}

pub(super) fn move_card(
    engines: &[Installation],
    saved: &mut Vec<String>,
    source: &str,
    target: &str,
) -> bool {
    let mut order = indices(engines, saved);
    let Some(from) = order
        .iter()
        .position(|&i| key(&engines[i].path) == key(source))
    else {
        return false;
    };
    let Some(to) = order
        .iter()
        .position(|&i| key(&engines[i].path) == key(target))
    else {
        return false;
    };
    if from == to {
        return false;
    }
    let moved = order.remove(from);
    order.insert(to, moved);
    *saved = order.into_iter().map(|i| engines[i].path.clone()).collect();
    true
}

pub(super) fn drag_response(ui: &mut Ui, rect: Rect, path: &str) -> Response {
    let response = ui.interact(
        rect,
        Id::new(("engine-card", path)),
        Sense::click_and_drag(),
    );
    response.dnd_set_drag_payload(EngineCardDrag { path: path.into() });
    if response.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    response
        .on_hover_cursor(CursorIcon::Grab)
        .on_hover_text("Drag to rearrange engine cards")
}

pub(super) fn paint_drag_label(ctx: &Context, engines: &[Installation], theme: &Theme) {
    let Some(drag) = DragAndDrop::payload::<EngineCardDrag>(ctx) else {
        return;
    };
    let Some(engine) = engines.iter().find(|e| e.path == drag.path) else {
        return;
    };
    let Some(pointer) = ctx.pointer_latest_pos() else {
        return;
    };
    let painter = ctx.layer_painter(LayerId::new(
        Order::Tooltip,
        Id::new("engine-card-drag-label"),
    ));
    let text = painter.layout_no_wrap(
        format!("Unreal Engine {}", engine.version),
        FontId::proportional(16.),
        color(theme.text),
    );
    let rect = Rect::from_min_size(pointer + vec2(16., 16.), text.size() + vec2(24., 20.));
    painter.rect(
        rect,
        5.,
        color(theme.card),
        Stroke::new(2., color(theme.accent_hover)),
        StrokeKind::Inside,
    );
    painter.galley(rect.min + vec2(12., 10.), text, color(theme.text));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn installations(paths: &[&str]) -> Vec<Installation> {
        paths
            .iter()
            .map(|p| Installation {
                path: (*p).into(),
                version: "5.8.4".into(),
                association: String::new(),
            })
            .collect()
    }
    #[test]
    fn order_survives_restart_rescan_and_missing_or_new_engines_without_changing_default() {
        let engines = installations(&["F:/UE_5.8", "F:/UE_5.7", "F:/UE_5.6"]);
        let mut prefs = Preferences {
            default_engine: engines[0].path.clone(),
            ..Default::default()
        };
        assert!(move_card(
            &engines,
            &mut prefs.engine_card_order,
            "f:\\UE_5.6\\",
            "F:/UE_5.8"
        ));
        assert_eq!(indices(&engines, &prefs.engine_card_order), vec![2, 0, 1]);
        let restored: Preferences =
            serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        assert_eq!(restored.default_engine, "F:/UE_5.8");
        let rescanned = installations(&["F:/UE_new", "f:/ue_5.7", "f:/ue_5.6", "f:/ue_5.8"]);
        assert_eq!(
            indices(&rescanned, &restored.engine_card_order),
            vec![2, 3, 1, 0]
        );
        let remaining = installations(&["F:/UE_new", "F:/UE_5.7"]);
        assert_eq!(indices(&remaining, &restored.engine_card_order), vec![1, 0]);
        assert!(move_card(
            &engines,
            &mut prefs.engine_card_order,
            "F:/UE_5.6",
            "F:/UE_5.7"
        ));
        assert_eq!(indices(&engines, &prefs.engine_card_order), vec![0, 1, 2]);
        let before = prefs.engine_card_order.clone();
        assert!(!move_card(
            &engines,
            &mut prefs.engine_card_order,
            "missing",
            "F:/UE_5.7"
        ));
        assert!(!move_card(
            &engines,
            &mut prefs.engine_card_order,
            "F:/UE_5.7",
            "F:/UE_5.7"
        ));
        assert_eq!(prefs.engine_card_order, before);
    }
    #[test]
    fn pointer_drag_across_rows_reorders_only_on_drop_and_preserves_button_clicks() {
        let ctx = Context::default();
        let engines = installations(&["A", "B"]);
        let mut saved = vec![];
        let frame = |events: Vec<Event>, saved: &mut Vec<String>| {
            let mut clicked = false;
            let _ = ctx.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500., 500.))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    CentralPanel::default().show(ctx, |ui| {
                        for (position, i) in indices(&engines, saved).into_iter().enumerate() {
                            let top = 10. + position as f32 * 180.;
                            let response = drag_response(
                                ui,
                                Rect::from_min_size(pos2(10., top), vec2(320., 140.)),
                                &engines[i].path,
                            );
                            if let Some(drag) = response.dnd_release_payload::<EngineCardDrag>() {
                                move_card(&engines, saved, &drag.path, &engines[i].path);
                            }
                            clicked |= ui
                                .interact(
                                    Rect::from_min_size(pos2(180., top + 100.), vec2(100., 32.)),
                                    Id::new(("launch", i)),
                                    Sense::click(),
                                )
                                .clicked();
                        }
                    });
                },
            );
            clicked
        };
        let press = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let start = pos2(40., 40.);
        let target = pos2(40., 230.);
        frame(vec![], &mut saved);
        frame(vec![Event::PointerMoved(start)], &mut saved);
        frame(vec![press(start, true)], &mut saved);
        frame(vec![Event::PointerMoved(target)], &mut saved);
        assert!(DragAndDrop::payload::<EngineCardDrag>(&ctx).is_some());
        assert!(
            saved.is_empty(),
            "Dragging alone must not persist a new order"
        );
        assert!(!frame(vec![press(target, false)], &mut saved));
        assert_eq!(saved, vec!["B", "A"]);
        assert!(DragAndDrop::payload::<EngineCardDrag>(&ctx).is_none());
        // A normal click on the overlapping Launch button keeps its behavior.
        let button = pos2(200., 120.);
        frame(vec![Event::PointerMoved(button)], &mut saved);
        frame(vec![press(button, true)], &mut saved);
        assert!(frame(vec![press(button, false)], &mut saved));
        assert_eq!(saved, vec!["B", "A"]);
        // Releasing outside the cards cancels the drag.
        frame(vec![Event::PointerMoved(start)], &mut saved);
        frame(vec![press(start, true)], &mut saved);
        let outside = pos2(440., 440.);
        frame(vec![Event::PointerMoved(outside)], &mut saved);
        frame(vec![press(outside, false)], &mut saved);
        assert_eq!(saved, vec!["B", "A"]);
    }
}
