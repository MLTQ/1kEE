use super::*;
use crate::model::{AppModel, EventSeverity, FactalBrief};

fn event(id: &str, lat: f32, lon: f32, minute: u8) -> EventRecord {
    EventRecord {
        id: format!("factal-{id}"),
        title: id.into(),
        summary: "Fixture".into(),
        severity: EventSeverity::Elevated,
        location_name: "Test location".into(),
        location: GeoPoint { lat, lon },
        source: "Factal".into(),
        occurred_at: "test".into(),
        factal_brief: Some(FactalBrief {
            factal_id: id.into(),
            severity_value: Some(2),
            occurred_at_raw: Some(format!("2026-09-28T12:{minute:02}:00Z")),
            point_wkt: None,
            vertical: None,
            subvertical: None,
            topics: vec![],
            content: None,
            raw_json_pretty: "{}".into(),
        }),
    }
}

#[test]
fn arrivals_ignore_repeated_pages_and_revisions_and_choose_newest_in_a_burst() {
    let a = event("a", 10.0, 20.0, 1);
    let b = event("b", 30.0, 40.0, 2);
    let c = event("c", 50.0, 60.0, 3);
    let mut f = EventFollow::default();
    let mut view = GlobeViewState::from_focus(GeoPoint { lat: 0.0, lon: 0.0 });
    f.observe(std::slice::from_ref(&a));
    f.enable(std::slice::from_ref(&a));
    assert_eq!(f.tick(&mut view, 0.0).as_deref(), Some("factal-a"));
    let mut revised = a.clone();
    revised.title = "Revised headline".into();
    f.observe(&[c.clone(), revised.clone(), b]);
    assert_eq!(f.tick(&mut view, 3.0).as_deref(), Some("factal-c"));
    f.observe(&[]);
    f.observe(&[revised, c]);
    assert_eq!(f.tick(&mut view, 4.0), None);
    assert_eq!(f.flight.as_ref().unwrap().started, 3.0);
}

#[test]
fn waits_for_real_factal_and_ignores_earthquakes_and_invalid_coordinates() {
    let mut f = EventFollow::default();
    let mut view = GlobeViewState::from_focus(GeoPoint { lat: 0.0, lon: 0.0 });
    f.enable(&[]);
    assert_eq!(f.tick(&mut view, 0.0), None);
    let mut quake = event("quake", 1.0, 1.0, 1);
    quake.factal_brief = None;
    f.observe(&[quake, event("bad", f32::NAN, 2.0, 2)]);
    assert_eq!(f.tick(&mut view, 1.0), None);
    f.observe(&[event("valid", 1.0, 2.0, 3)]);
    assert_eq!(f.tick(&mut view, 2.0).as_deref(), Some("factal-valid"));
}

#[test]
fn near_and_far_flights_arrive_in_ten_seconds_in_both_views() {
    for local in [false, true] {
        for destination in [
            GeoPoint {
                lat: 11.0,
                lon: 21.0,
            },
            GeoPoint {
                lat: -40.0,
                lon: -150.0,
            },
        ] {
            let mut view = GlobeViewState::from_focus(GeoPoint {
                lat: 10.0,
                lon: 20.0,
            });
            view.local_mode = local;
            let f = Flight::new(&view, [0.0; 5], destination, 100.0);
            let (before, _) = f.sample(109.0);
            let (at, v) = f.sample(110.0);
            assert!((at[0] - f.end[0]).abs() < 1e-8);
            assert!((at[1] - f.end[1]).abs() < 1e-8);
            assert!((before[0] - f.end[0]).abs() > 0.0001);
            assert_eq!(v, f.final_velocity);
            for step in 0..1001 {
                let (p, v) = f.sample(100.0 + step as f64 / 100.0);
                assert!(p.iter().chain(&v).all(|x| x.is_finite()));
                apply(&mut view, p);
                assert_eq!(view.local_mode, local);
            }
        }
    }
}

#[test]
fn date_line_uses_short_route_and_polar_orbits_stay_bounded() {
    for local in [false, true] {
        let mut view = GlobeViewState::from_focus(GeoPoint {
            lat: 0.0,
            lon: 179.0,
        });
        view.local_mode = local;
        let f = Flight::new(
            &view,
            [0.0; 5],
            GeoPoint {
                lat: 0.0,
                lon: -179.0,
            },
            0.0,
        );
        assert!((f.end[1] - f.start[1] - 2.0).abs() < 0.0001);
        assert!((f.sample(5.0).0[1].abs() - 180.0).abs() < 0.2);
        for lat in [-90.0, 90.0] {
            let f = Flight::new(&view, [0.0; 5], GeoPoint { lat, lon: -1.0 }, 0.0);
            for t in 0..400 {
                apply(&mut view, f.sample(t as f64).0);
                assert!(view.local_center.lat.abs() <= 85.0);
                assert!(view.local_center.lon.abs() <= 180.0);
            }
        }
    }
}

#[test]
fn retarget_preserves_position_and_velocity_during_flight_and_orbit() {
    for local in [false, true] {
        for retarget_at in [4.0, 25.0] {
            let mut view = GlobeViewState::from_focus(GeoPoint {
                lat: 40.0,
                lon: -70.0,
            });
            view.local_mode = local;
            let mut f = EventFollow::default();
            f.enable(&[event("a", -20.0, 100.0, 1)]);
            f.tick(&mut view, 0.0);
            let (old, velocity) = f.flight.as_ref().unwrap().sample(retarget_at);
            f.observe(&[event("b", 60.0, -10.0, 2)]);
            f.tick(&mut view, retarget_at);
            let (new, new_velocity) = f.flight.as_ref().unwrap().sample(retarget_at);
            for i in 0..5 {
                let difference = if i == 1 {
                    wrap(old[i] - new[i])
                } else {
                    old[i] - new[i]
                };
                assert!(difference.abs() < 0.0001, "axis {i}: {old:?} vs {new:?}");
                assert!((velocity[i] - new_velocity[i]).abs() < 1e-8);
            }
        }
    }
}

#[test]
fn zoom_join_and_orbit_arrival_have_continuous_velocity() {
    for local in [false, true] {
        let mut view = GlobeViewState::from_focus(GeoPoint {
            lat: 40.0,
            lon: -70.0,
        });
        view.local_mode = local;
        let f = Flight::new(
            &view,
            [0.0; 5],
            GeoPoint {
                lat: -20.0,
                lon: 100.0,
            },
            0.0,
        );
        for t in [3.0, 10.0] {
            let (a, va) = f.sample(t - 0.00001);
            let (b, vb) = f.sample(t + 0.00001);
            for i in 0..5 {
                assert!((a[i] - b[i]).abs() < 0.002);
                assert!((va[i] - vb[i]).abs() < 0.002);
            }
        }
    }
}

#[test]
fn model_refresh_does_not_snap_camera_or_lose_followed_brief() {
    let mut model = AppModel::new();
    model.globe_view.local_mode = true;
    let origin = model.globe_view.local_center;
    model.toggle_event_follow();
    model.replace_factal_events(vec![event("a", 40.0, 120.0, 1)]);
    assert_eq!(model.globe_view.local_center, origin);
    model.tick_event_follow(0.0);
    assert_eq!(model.globe_view.local_center, origin);
    assert!(model.factal_brief_open);
    model.tick_event_follow(5.0);
    let flying = model.globe_view.local_center;
    model.replace_factal_events(vec![]);
    model.replace_usgs_events(vec![]);
    assert_eq!(model.globe_view.local_center, flying);
    assert_eq!(model.selected_event().unwrap().id, "factal-a");
    model.tick_event_follow(10.0);
    assert_eq!(
        model.globe_view.local_center,
        GeoPoint {
            lat: 40.0,
            lon: 120.0
        }
    );
    model.stop_event_follow();
    let stopped = model.globe_view.local_yaw;
    model.tick_event_follow(60.0);
    assert_eq!(model.globe_view.local_yaw, stopped);
}

#[test]
fn replay_and_manual_selection_release_follow_control() {
    let mut model = AppModel::new();
    model.replace_factal_events(vec![event("a", 10.0, 20.0, 1)]);
    model.toggle_event_follow();
    model.tick_event_follow(0.0);
    model.select_event("factal-a");
    assert!(!model.event_follow.enabled());
    model.toggle_event_follow();
    model.tick_event_follow(1.0);
    model.replay_mode = true;
    model.tick_event_follow(2.0);
    assert!(!model.event_follow.enabled());
}

#[test]
fn cinematic_brief_renders_the_followed_record_to_the_right_of_map_center() {
    let mut model = AppModel::new();
    model.replace_factal_events(vec![event("followed-headline", 10.0, 20.0, 1)]);
    model.toggle_event_follow();
    model.tick_event_follow(0.0);
    model.replace_factal_events(vec![]);
    assert!(model.cinematic_mode);
    let ctx = egui::Context::default();
    let mut title_position = None;
    for frame in 0..3 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                time: Some(frame as f64 * 0.1),
                ..Default::default()
            },
            |ctx| crate::panels::render_factal_brief(ctx, &mut model),
        );
        for shape in output.shapes {
            if let egui::Shape::Text(t) = shape.shape {
                if t.galley.text() == "followed-headline" {
                    title_position = Some(t.pos);
                }
            }
        }
    }
    let position = title_position.expect("followed headline rendered after leaving live page");
    assert!(position.x > 600.0 && position.x < 1180.0, "{position:?}");
    assert!(position.y > 60.0 && position.y < 300.0, "{position:?}");
}
