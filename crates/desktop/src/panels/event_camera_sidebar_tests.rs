use super::*;
use crate::model::{EventRecord, EventSeverity, FactalBrief};
use std::sync::atomic::Ordering;

fn camera(id: &str, lon: f32) -> CameraFeed {
    CameraFeed {
        id: id.into(),
        label: format!("Camera {id}"),
        provider: "Public camera fixture".into(),
        kind: "snapshot".into(),
        location: GeoPoint { lat: 0.0, lon },
        stream_url: format!("https://camera.test/{id}.jpg"),
        last_seen: String::new(),
        status: CameraConnectionState::Reachable,
    }
}

fn model() -> AppModel {
    let mut model = AppModel::new();
    model.replace_factal_events(vec![EventRecord {
        id: "factal-sidebar".into(),
        title: "Focused event fixture".into(),
        summary: "A sample report used to verify the camera sidebar layout.".into(),
        severity: EventSeverity::Elevated,
        location_name: "Event location".into(),
        location: GeoPoint { lat: 0.0, lon: 0.0 },
        source: "Fixture".into(),
        occurred_at: "Test time".into(),
        factal_brief: Some(FactalBrief {
            factal_id: "sidebar".into(),
            severity_value: Some(2),
            occurred_at_raw: None,
            point_wkt: None,
            vertical: None,
            subvertical: None,
            topics: vec![],
            content: None,
            raw_json_pretty: "{}".into(),
        }),
    }]);
    model.replace_camera_registry(
        (0..8).map(|i| camera(&i.to_string(), i as f32)).collect(),
        "Fixture",
    );
    model.toggle_event_follow();
    model.tick_event_follow(0.0);
    model
}

fn prime(ctx: &egui::Context, sidebar: &mut EventCameraSidebar, model: &AppModel) {
    sidebar.update_selection(model, model.selected_event().map(|event| event.location));
    for ranked in &sidebar.cameras {
        let image = egui::ColorImage::new([320, 180], egui::Color32::from_rgb(45, 70, 90));
        sidebar.players.insert(
            ranked.camera.id.clone(),
            CameraFeedPlayer::fixture(ctx, &ranked.camera, image).0,
        );
    }
}

fn frame(
    ctx: &egui::Context,
    sidebar: &mut EventCameraSidebar,
    model: &mut AppModel,
    width: f32,
) -> (egui::FullOutput, egui::Rect) {
    let mut map = egui::Rect::NOTHING;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 720.0),
            )),
            ..Default::default()
        },
        |ctx| {
            sidebar.render(ctx, model);
            super::super::factal_brief::render_factal_brief(ctx, model);
            egui::CentralPanel::default().show(ctx, |ui| {
                map = ui.max_rect();
                ui.painter().rect_filled(map, 0, theme::canvas_background());
                if !model.obs_view {
                    super::super::map_rail::render(ui, model);
                }
            });
        },
    );
    (output, map)
}

fn text_shapes(output: &egui::FullOutput) -> Vec<(String, egui::Rect, egui::Rect)> {
    fn collect(
        shape: &egui::Shape,
        clip: egui::Rect,
        out: &mut Vec<(String, egui::Rect, egui::Rect)>,
    ) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.text().to_owned(),
                shape.visual_bounding_rect(),
                clip,
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, clip, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, shape.clip_rect, &mut out);
    }
    out
}

#[test]
fn nearest_feeds_use_great_circle_distance_and_exclude_unusable_endpoints() {
    let mut cameras = vec![
        camera("west", -179.8),
        camera("east", 179.9),
        camera("far", 0.0),
    ];
    for (index, url) in [
        "",
        "rtsp://camera.test/live",
        "https://user:pass@camera.test/a.jpg",
        "https://camera.test/a.m3u8",
    ]
    .iter()
    .enumerate()
    {
        let mut invalid = camera(&format!("invalid{index}"), 179.95);
        invalid.stream_url = (*url).into();
        cameras.push(invalid);
    }
    let mut unreachable = camera("offline", 179.95);
    unreachable.status = CameraConnectionState::Unreachable;
    cameras.push(unreachable);
    let mut invalid_location = camera("invalid-location", 0.0);
    invalid_location.location.lat = 100.0;
    cameras.push(invalid_location);
    let ranked = nearest_feeds(
        &cameras,
        GeoPoint {
            lat: 0.0,
            lon: 179.95,
        },
    );
    assert_eq!(
        ranked
            .iter()
            .map(|r| r.camera.id.as_str())
            .collect::<Vec<_>>(),
        ["east", "west", "far"]
    );
    assert!(ranked[0].distance_km < 10.0);
    assert!(ranked[1].distance_km < 30.0);
    assert!(
        ranked[2].distance_km > 10_000.0,
        "closest available has no arbitrary radius cutoff"
    );
    let tied = nearest_feeds(
        &[camera("b", 0.0), camera("a", 0.0)],
        GeoPoint { lat: 0.0, lon: 0.0 },
    );
    assert_eq!(tied[0].camera.id, "a");
}

#[test]
fn event_and_registry_changes_retarget_four_feeds_and_preserve_overlapping_players() {
    let ctx = egui::Context::default();
    let mut model = model();
    let mut sidebar = EventCameraSidebar::default();
    prime(&ctx, &mut sidebar, &model);
    let initial_map_snapshot = model.nearby_camera_snapshot(250.0);
    let texture = sidebar.players["3"].texture.as_ref().unwrap().id();
    let (old_player, stop) = CameraFeedPlayer::fixture(
        &ctx,
        &sidebar.cameras[0].camera,
        egui::ColorImage::new([1, 1], egui::Color32::WHITE),
    );
    sidebar.players.insert("0".into(), old_player);
    model.events[0].location.lon = 4.0;
    sidebar.update_selection(&model, model.selected_event().map(|e| e.location));
    assert_eq!(
        sidebar
            .cameras
            .iter()
            .map(|r| r.camera.id.as_str())
            .collect::<Vec<_>>(),
        ["4", "3", "5", "2"]
    );
    assert_eq!(sidebar.players.len(), MAX_FEEDS);
    assert!(stop.load(Ordering::Acquire));
    assert_eq!(sidebar.players["3"].texture.as_ref().unwrap().id(), texture);
    model.events[0].location.lon = 0.0;
    sidebar.update_selection(&model, model.selected_event().map(|e| e.location));
    assert!(
        std::sync::Arc::ptr_eq(&initial_map_snapshot, &model.nearby_camera_snapshot(250.0)),
        "sidebar must not evict map proximity cache"
    );
    model.replace_camera_registry(vec![camera("replacement", 0.0)], "Updated fixture");
    sidebar.update_selection(&model, model.selected_event().map(|e| e.location));
    assert_eq!(sidebar.cameras.len(), 1);
    assert!(sidebar.players.contains_key("replacement"));
    model.stop_event_follow();
    let _ = frame(&ctx, &mut sidebar, &mut model, 1100.0);
    assert!(sidebar.players.is_empty());
    assert!(sidebar.cameras.is_empty());
}

#[test]
fn sidebar_is_a_single_right_column_and_brief_stays_beside_it_in_normal_and_obs_views() {
    for width in [1100.0, 1520.0] {
        for obs in [false, true] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let mut model = model();
            model.obs_view = obs;
            model.cameras[0].label = "A very long camera name ".repeat(15);
            let mut sidebar = EventCameraSidebar::default();
            prime(&ctx, &mut sidebar, &model);
            for _ in 0..3 {
                let _ = frame(&ctx, &mut sidebar, &mut model, width);
            }
            let (output, map) = frame(&ctx, &mut sidebar, &mut model, width);
            let labels = text_shapes(&output);
            let header = labels
                .iter()
                .find(|(text, _, _)| text == "NEAREST CAMERAS")
                .unwrap();
            assert!(header.1.left() > map.right());
            assert!(map.width() > width - 350.0);
            let cards: Vec<_> = labels
                .iter()
                .filter(|(text, _, _)| {
                    text.starts_with("01  ")
                        || text.starts_with("02  ")
                        || text.starts_with("03  ")
                        || text.starts_with("04  ")
                })
                .collect();
            assert!(
                cards.len() >= 2,
                "at least two cards fit; remaining feeds scroll"
            );
            for pair in cards.windows(2) {
                assert!((pair[0].1.left() - pair[1].1.left()).abs() < 1.0);
                assert!(pair[0].1.bottom() < pair[1].1.top());
            }
            let brief = ctx
                .memory(|memory| memory.area_rect(egui::Id::new("event-follow-brief")))
                .unwrap();
            assert!(
                brief.right() <= map.right() + 1.0,
                "brief overlaps feed sidebar: {brief:?} {map:?}"
            );
            assert!(!model.camera_feed_window_open);
            assert_eq!(sidebar.players.len(), MAX_FEEDS);
        }
    }
}

#[test]
fn waiting_and_empty_states_do_not_start_players_and_retained_tour_targets_still_work() {
    let ctx = egui::Context::default();
    let mut model = AppModel::new();
    model.toggle_event_follow();
    let mut sidebar = EventCameraSidebar::default();
    let (output, _) = frame(&ctx, &mut sidebar, &mut model, 1100.0);
    assert!(
        text_shapes(&output)
            .iter()
            .any(|(text, _, _)| text == "Waiting for a live event to follow.")
    );
    assert!(sidebar.players.is_empty());
    model = self::model();
    prime(&ctx, &mut sidebar, &model);
    model.replace_factal_events(vec![]);
    let _ = frame(&ctx, &mut sidebar, &mut model, 1100.0);
    assert_eq!(
        sidebar.players.len(),
        MAX_FEEDS,
        "followed event is retained after paging out"
    );
    model.replace_camera_registry(vec![], "Empty fixture");
    let (output, _) = frame(&ctx, &mut sidebar, &mut model, 1100.0);
    assert!(sidebar.players.is_empty());
    assert!(
        text_shapes(&output)
            .iter()
            .any(|(text, _, _)| text == "No camera feeds available for this event.")
    );
}

#[cfg(test)]
#[path = "event_camera_sidebar_preview.rs"]
mod preview;
