use super::*;
use crate::model::{EventRecord, EventSeverity, FactalBrief, GeoPoint};

fn event(index: usize) -> EventRecord {
    EventRecord {
        id: format!("factal-{index}"),
        title: format!("Story {index}"),
        summary: format!("Full event summary {index}."),
        severity: EventSeverity::Critical,
        location_name: "Fixture".into(),
        location: GeoPoint { lat: 0.0, lon: 0.0 },
        source: "Factal".into(),
        occurred_at: format!("2026-10-31 12:{index:02}:00"),
        factal_brief: Some(FactalBrief {
            factal_id: index.to_string(),
            severity_value: Some(5),
            occurred_at_raw: Some(format!("2026-10-31T12:{index:02}:00Z")),
            point_wkt: None,
            vertical: None,
            subvertical: None,
            topics: vec![],
            content: None,
            raw_json_pretty: "{}".into(),
        }),
    }
}

fn model() -> AppModel {
    let mut model = AppModel::new();
    model.events = (0..8).map(event).collect();
    model.toggle_event_follow();
    model.tick_event_follow(0.0);
    model
}

#[test]
fn globe_local_and_focus_show_five_crawls_and_only_the_open_briefs_selected_card() {
    for local in [false, true] {
        for focus in [false, true] {
            for open in [false, true] {
                let ctx = egui::Context::default();
                let mut model = model();
                model.globe_view.local_mode = local;
                model.obs_view = focus;
                model.factal_brief_open = open;
                let viewport =
                    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
                let markers: Vec<_> = model
                    .events
                    .iter()
                    .enumerate()
                    .map(|(index, event)| {
                        let base = egui::pos2(200.0 + index as f32 * 50.0, 300.0);
                        marker_style::MapMarker::new(&event.id, base, base, 1.0)
                    })
                    .collect();
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(viewport),
                        time: Some(15.0),
                        ..Default::default()
                    },
                    |ctx| {
                        draw_event_follow_overlay(
                            &ctx.layer_painter(egui::LayerId::background()),
                            viewport,
                            &model,
                            &markers,
                        );
                    },
                );
                let text: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some(text.galley.text()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    text.iter()
                        .filter(|text| text.contains(" - Critical - "))
                        .count(),
                    5
                );
                let selected = model.selected_event().unwrap();
                assert_eq!(
                    text.iter().filter(|text| **text == selected.title).count(),
                    usize::from(open)
                );
                for event in model.events.iter().filter(|event| event.id != selected.id) {
                    assert!(!text.contains(&event.title.as_str()));
                }
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape
                        && text.galley.text() == selected.title
                    {
                        assert!(
                            text.pos.y + text.galley.size().y
                                < viewport.bottom() - super::super::event_chyron::HEIGHT - 12.0
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn leaving_follow_removes_the_entire_overlay() {
    let ctx = egui::Context::default();
    let mut model = model();
    model.stop_event_follow();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        },
        |ctx| {
            draw_event_follow_overlay(
                &ctx.layer_painter(egui::LayerId::background()),
                rect,
                &model,
                &[],
            );
        },
    );
    assert!(output.shapes.is_empty());
}

#[test]
fn a_long_follow_brief_keeps_the_bottom_chyron_and_local_footer_clear() {
    for height in [720.0, 800.0, 1080.0] {
        let ctx = egui::Context::default();
        let mut model = model();
        let id = model.selected_event_id.clone().unwrap();
        model
            .events
            .iter_mut()
            .find(|event| event.id == id)
            .unwrap()
            .summary = "A long summary needs internal scrolling. ".repeat(400);
        for frame in 0..4 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, height),
                    )),
                    time: Some(frame as f64 * 0.1),
                    ..Default::default()
                },
                |ctx| super::super::render_factal_brief(ctx, &mut model),
            );
        }
        let brief = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("event-follow-brief")))
            .unwrap();
        let chyron_top = height - 14.0 - 82.0 - super::super::event_chyron::HEIGHT - 12.0;
        assert!(
            brief.bottom() < chyron_top,
            "height {height}: {brief:?}, chyron top {chyron_top}"
        );
    }
}

#[path = "event_overlay_preview.rs"]
mod preview;
