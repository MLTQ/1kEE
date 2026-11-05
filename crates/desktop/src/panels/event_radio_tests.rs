use super::*;
use crate::model::{EventSeverity, GeoPoint};

#[path = "event_radio_preview.rs"]
mod preview;

fn event(id: &str, lat: f32, lon: f32) -> EventRecord {
    EventRecord {
        id: id.into(),
        title: format!("Fixture event {id}"),
        summary: String::new(),
        severity: EventSeverity::Advisory,
        location_name: "Kamloops".into(),
        location: GeoPoint { lat, lon },
        source: "Fixture".into(),
        occurred_at: String::new(),
        factal_brief: None,
    }
}
fn feed(id: &str, location: Option<(f64, f64)>) -> RadioFeed {
    RadioFeed {
        id: id.into(),
        name: id.into(),
        url: format!("https://radio.example.org/{id}.mp3"),
        homepage: String::new(),
        kinds: vec![RadioKind::Fire],
        latitude: location.map(|p| p.0),
        longitude: location.map(|p| p.1),
        locality: "Fixture".into(),
        directory_id: None,
    }
}
fn primed() -> EventRadio {
    EventRadio {
        loaded: true,
        fetched_at: Some(Instant::now()),
        catalog: vec![
            feed("Local fire dispatch", Some((50.67, -120.34))),
            feed("Distant dispatch", Some((40.0, 20.0))),
            feed("Unmapped feed", None),
        ],
        ..Default::default()
    }
}

#[test]
fn nearby_and_all_scopes_do_not_misrepresent_unknown_or_distant_feeds() {
    let mut radio = primed();
    let area = RadioArea::from(&event("a", 50.67, -120.34));
    assert_eq!(radio.visible_feeds(&area).len(), 1);
    radio.browse_all = true;
    assert_eq!(radio.visible_feeds(&area).len(), 3);
    radio.kind = Some(RadioKind::Police);
    assert!(radio.visible_feeds(&area).is_empty());
    radio.kind = None;
    radio.filter = "unmapped".into();
    assert_eq!(radio.visible_feeds(&area).len(), 1);
}

#[test]
fn event_context_is_pinned_and_explicit_retarget_clears_playback() {
    let ctx = egui::Context::default();
    let mut radio = primed();
    radio.open_for(&ctx, &event("a", 50.67, -120.34));
    assert!(radio.open);
    assert!(radio.pending.is_none());
    radio.playing = Some(feed("old feed", None));
    // A normal render with a different selection does not change the searched area.
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            radio.contents(
                ui,
                &radio.area.clone().unwrap(),
                Some(&event("b", 10.0, 10.0)),
            )
        });
    });
    assert_eq!(radio.area.as_ref().unwrap().event_id, "a");
    radio.open_for(&ctx, &event("b", 10.0, 10.0));
    assert!(radio.playing.is_none());
    assert_eq!(radio.area.as_ref().unwrap().event_id, "b");
}

#[test]
fn closing_cancels_pending_work_and_discards_late_results() {
    let mut radio = primed();
    let (sender, receiver) = crossbeam_channel::bounded(1);
    let cancel = Arc::new(AtomicBool::new(false));
    radio.pending = Some(Search {
        receiver,
        cancel: Arc::clone(&cancel),
    });
    radio.playing = Some(feed("old feed", None));
    radio.open = true;
    radio.close();
    assert!(cancel.load(Ordering::Acquire));
    assert!(!radio.open);
    assert!(radio.playing.is_none());
    assert!(
        sender
            .try_send(Ok(catalog::CatalogReport {
                feeds: Vec::new(),
                warning: None
            }))
            .is_err()
    );
    assert_eq!(radio.catalog.len(), 3);
}

#[test]
fn headless_window_renders_empty_and_populated_states_without_network_or_audio() {
    let ctx = egui::Context::default();
    let mut radio = primed();
    radio.open_for(&ctx, &event("a", 50.67, -120.34));
    let area = radio.area.clone().unwrap();
    for empty in [false, true] {
        if empty {
            radio.kind = Some(RadioKind::Ems);
        }
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 800.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| radio.contents(ui, &area, None));
            },
        );
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(t) => Some(t.galley.text()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            text.contains(if empty {
                "No open feeds match"
            } else {
                "Local fire dispatch"
            }),
            "{text}"
        );
        assert!(!text.contains("Distant dispatch"));
    }
}

#[test]
fn nearby_radio_button_opens_the_clicked_event_without_autoplay() {
    let ctx = egui::Context::default();
    let mut radio = primed();
    let event = event("clicked", 50.67, -120.34);
    let output = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| radio.button(ui, &event));
    });
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Nearby radio" => {
                Some(text.pos + text.galley.size() / 2.0)
            }
            _ => None,
        })
        .expect("radio button");
    for pressed in [true, false] {
        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| radio.button(ui, &event));
            },
        );
    }
    assert!(radio.open);
    assert_eq!(radio.area.as_ref().unwrap().event_id, "clicked");
    assert!(radio.playing.is_none());
    assert!(radio.pending.is_none());
}
