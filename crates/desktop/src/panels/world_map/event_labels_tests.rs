use super::*;
use crate::model::{EventSeverity, GeoPoint};

#[test]
fn clustered_events_get_separate_cards_without_leaving_the_map() {
    let bounds = Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 650.0));
    let mut occupied = Vec::new();
    for _ in 0..8 {
        let card = place_card(bounds.center(), egui::vec2(180.0, 85.0), bounds, &occupied);
        assert!(bounds.contains_rect(card));
        assert!(occupied.iter().all(|other| !other.intersects(card)));
        occupied.push(card);
    }
}

#[test]
fn edge_cards_remain_inside_the_viewport() {
    let bounds = Rect::from_min_max(egui::pos2(100.0, 80.0), egui::pos2(700.0, 480.0));
    for anchor in [
        bounds.left_top(),
        bounds.right_top(),
        bounds.left_bottom(),
        bounds.right_bottom(),
    ] {
        let card = place_card(anchor, egui::vec2(220.0, 100.0), bounds, &[]);
        assert!(bounds.contains_rect(card));
    }
}

#[test]
fn all_visible_event_cards_render_without_a_pointer_or_tour_rank_limit() {
    let ctx = egui::Context::default();
    let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let events: Vec<_> = (0..10)
        .map(|index| EventRecord {
            id: format!("event-{index}"),
            title: format!("Regional headline {index}"),
            summary: String::new(),
            severity: EventSeverity::Elevated,
            location_name: "Nearby location".into(),
            location: GeoPoint { lat: 0.0, lon: 0.0 },
            source: String::new(),
            occurred_at: String::new(),
            factal_brief: None,
        })
        .collect();
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
    for event in &events[..8] {
        assert_eq!(text.iter().filter(|text| **text == event.title).count(), 1);
    }
    for event in &events[8..] {
        assert!(!text.contains(&event.title.as_str()));
    }
    assert_eq!(text.iter().filter(|text| **text == "Elevated").count(), 8);
}
