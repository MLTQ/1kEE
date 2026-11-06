use super::*;
use crate::model::{EventSeverity, GeoPoint};
use egui::Pos2;

fn event(index: usize) -> EventRecord {
    EventRecord {
        id: format!("event-{index}"),
        title: format!("Regional headline {index}"),
        summary: String::new(),
        severity: EventSeverity::Elevated,
        location_name: "Nearby location".into(),
        location: GeoPoint { lat: 0.0, lon: 0.0 },
        source: String::new(),
        occurred_at: String::new(),
        factal_brief: None,
    }
}

#[test]
fn hidden_cards_skip_event_lookup_and_emit_no_shapes() {
    let ctx = egui::Context::default();
    let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(viewport),
            ..Default::default()
        },
        |ctx| {
            draw(
                &ctx.layer_painter(egui::LayerId::background()),
                viewport,
                std::iter::once_with(|| -> &EventRecord {
                    panic!("hidden cards must not inspect events")
                }),
                &[],
                0.0,
                Some("event-0"),
            );
        },
    );
    assert!(output.shapes.is_empty());
}

#[test]
fn only_the_selected_event_card_renders_without_a_pointer() {
    let ctx = egui::Context::default();
    let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let events: Vec<_> = (0..10).map(event).collect();
    let markers: Vec<_> = events
        .iter()
        .take(9)
        .enumerate()
        .map(|(index, event)| {
            let base = if index == 8 {
                egui::pos2(-30.0, 200.0) // projected, but outside the current map
            } else {
                egui::pos2(
                    100.0 + (index % 4) as f32 * 300.0,
                    200.0 + (index / 4) as f32 * 350.0,
                )
            };
            MapMarker::new(&event.id, base, base, 1.0)
        })
        .collect();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(viewport),
            ..Default::default()
        },
        |ctx| {
            assert!(ctx.pointer_hover_pos().is_none());
            draw(
                &ctx.layer_painter(egui::LayerId::background()),
                viewport,
                events.iter(),
                &markers,
                1.0,
                Some("event-3"),
            );
        },
    );
    let text: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect();
    for (index, event) in events.iter().enumerate() {
        assert_eq!(
            text.iter().filter(|text| **text == event.title).count(),
            usize::from(index == 3)
        );
    }
    assert_eq!(text.iter().filter(|text| **text == "Elevated").count(), 1);
}

#[test]
fn moving_cluster_keeps_painted_cards_fixed_and_leaders_attached_until_hidden_travel() {
    let ctx = egui::Context::default();
    let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let events: Vec<_> = (0..4)
        .map(|index| {
            let mut event = event(index);
            event.title = "Yemen crisis".into();
            if index < 3 {
                event.severity = EventSeverity::Critical;
            }
            event
        })
        .collect();
    let mut initial = Vec::new();
    for frame in 0..122 {
        let markers: Vec<_> = events
            .iter()
            .enumerate()
            .map(|(index, event)| {
                let phase = frame as f32 * 0.01 + index as f32 * 0.001;
                let base = egui::pos2(
                    if frame == 121 {
                        1000.0
                    } else {
                        500.0 + phase.sin() * 35.0
                    },
                    360.0 + phase.cos() * 20.0,
                );
                MapMarker::new(&event.id, base, base, 1.0)
            })
            .collect();
        let opacity = if frame == 120 { 0.0 } else { 1.0 };
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(viewport),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            },
            |ctx| {
                draw(
                    &ctx.layer_painter(egui::LayerId::background()),
                    viewport,
                    events.iter(),
                    &markers,
                    opacity,
                    Some("event-0"),
                )
            },
        );
        let cards: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) => Some(rect.rect),
                _ => None,
            })
            .collect();
        let anchors: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::LineSegment { points, .. } => Some(points[0]),
                _ => None,
            })
            .collect();
        if frame == 120 {
            assert!(cards.is_empty());
            assert!(anchors.is_empty());
            continue;
        }
        assert_eq!(cards.len(), 1, "frame {frame}");
        assert_eq!(anchors, vec![markers[0].base]);
        if frame == 0 {
            initial = cards;
        } else if frame == 121 {
            assert_ne!(
                cards, initial,
                "a new arrival should receive a fresh layout"
            );
        } else {
            assert_eq!(cards, initial, "cards rearranged on frame {frame}");
        }
    }
}

#[test]
fn a_closed_brief_or_an_offscreen_selected_marker_paints_no_cards() {
    for (selected, base) in [
        (None, egui::pos2(200.0, 200.0)),
        (Some("event-0"), egui::pos2(-100.0, 200.0)),
    ] {
        let ctx = egui::Context::default();
        let event = event(0);
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let marker = MapMarker::new(&event.id, base, base, 1.0);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            },
            |ctx| {
                draw(
                    &ctx.layer_painter(egui::LayerId::background()),
                    viewport,
                    std::iter::once(&event),
                    std::slice::from_ref(&marker),
                    1.0,
                    selected,
                );
            },
        );
        assert!(output.shapes.is_empty());
    }
}
