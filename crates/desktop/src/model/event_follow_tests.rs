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
fn unchanged_refresh_preserves_the_current_stop_and_newest_breaks_severity_ties() {
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
fn travel_time_scales_with_distance_and_never_exceeds_ten_seconds() {
    for local in [false, true] {
        let mut view = GlobeViewState::from_focus(GeoPoint {
            lat: 10.0,
            lon: 20.0,
        });
        view.local_mode = local;
        // Start at cruising zoom so this isolates geographic travel time.
        view.local_zoom = 9.5;
        view.zoom = 35.0;
        let destinations = [
            GeoPoint {
                lat: if local { 10.001 } else { 9.201 },
                lon: 20.001,
            },
            GeoPoint {
                lat: 20.0,
                lon: 30.0,
            },
            GeoPoint {
                lat: -40.0,
                lon: -150.0,
            },
        ];
        let flights: Vec<_> = destinations
            .iter()
            .map(|&p| Flight::new(&view, [0.0; 5], p, 100.0))
            .collect();
        assert!(flights[0].duration < 1.0);
        assert!(flights[0].duration < flights[1].duration);
        assert!(flights[1].duration < flights[2].duration);
        assert_eq!(flights[2].duration, MAX_FLIGHT_SECONDS);
        for f in flights {
            let (at, velocity) = f.sample(100.0 + f.duration);
            for i in 0..5 {
                assert!((at[i] - f.end[i]).abs() < 1e-8);
                assert!((velocity[i] - f.final_velocity[i]).abs() < 1e-8);
            }
            for step in 0..1001 {
                let (p, v) = f.sample(100.0 + f.duration * step as f64 / 1000.0);
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
        assert!((f.sample(f.duration * 0.5).0[1].abs() - 180.0).abs() < 0.2);
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
        for location in [
            GeoPoint {
                lat: -20.0,
                lon: 100.0,
            },
            GeoPoint {
                lat: 43.0,
                lon: -65.0,
            },
        ] {
            let f = Flight::new(&view, [0.0; 5], location, 0.0);
            assert!(f.zoom_out);
            for t in [f.duration * 0.3, f.duration] {
                let (a, va) = f.sample(t - 0.00001);
                let (b, vb) = f.sample(t + 0.00001);
                for i in 0..5 {
                    assert!((a[i] - b[i]).abs() < 0.002);
                    assert!((va[i] - vb[i]).abs() < 0.002);
                }
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

fn severity_event(id: &str, severity: i64, minute: u8) -> EventRecord {
    let mut e = event(id, 10.0 + minute as f32 * 0.001, 20.0, minute);
    e.factal_brief.as_mut().unwrap().severity_value = Some(severity);
    e.severity = if severity >= 4 {
        EventSeverity::Critical
    } else if severity >= 2 {
        EventSeverity::Elevated
    } else {
        EventSeverity::Advisory
    };
    e
}

fn finish_stop(f: &mut EventFollow, view: &mut GlobeViewState) -> String {
    let flight = f.flight.as_ref().unwrap();
    let deadline = flight.started + flight.duration + ORBIT_SECONDS;
    assert_eq!(
        f.tick(view, deadline - 0.001),
        None,
        "orbit dwell shortened"
    );
    f.tick(view, deadline).expect("next tour stop")
}

#[test]
fn tour_takes_six_unique_events_by_numeric_severity_then_recency_and_cycles() {
    let mut fallback = severity_event("fallback", 4, 10);
    fallback.factal_brief.as_mut().unwrap().severity_value = None;
    let mut invalid = severity_event("invalid", 99, 50);
    invalid.location.lat = f32::NAN;
    let mut quake = severity_event("quake", 99, 51);
    quake.factal_brief = None;
    let events = vec![
        severity_event("low", 1, 59),
        severity_event("five-old", 5, 1),
        severity_event("four", 4, 1),
        severity_event("three", 3, 50),
        severity_event("two", 2, 59),
        severity_event("five-new", 5, 2),
        severity_event("five-new", 5, 2),
        fallback,
        invalid,
        quake,
    ];
    let expected = [
        "factal-five-new",
        "factal-five-old",
        "factal-fallback",
        "factal-four",
        "factal-three",
        "factal-two",
    ];
    let mut f = EventFollow::default();
    let mut view = GlobeViewState::from_focus(GeoPoint {
        lat: 10.0,
        lon: 20.0,
    });
    f.enable(&events);
    assert_eq!(f.tick(&mut view, 0.0).as_deref(), Some(expected[0]));
    for (i, id) in expected.iter().enumerate().skip(1) {
        assert_eq!(finish_stop(&mut f, &mut view), *id);
        assert_eq!(f.tour_position(), Some((i + 1, 6)));
        // Reordered identical payloads must not reset the tour to its head.
        let reversed: Vec<_> = events.iter().rev().cloned().collect();
        f.observe(&reversed);
        assert_eq!(
            f.tick(&mut view, f.flight.as_ref().unwrap().started + 0.01),
            None
        );
    }
    assert_eq!(finish_stop(&mut f, &mut view), expected[0]);
}

#[test]
fn payload_severity_revision_reprioritizes_existing_ids_and_replaces_stale_stops() {
    let a = severity_event("a", 4, 1);
    let b = severity_event("b", 2, 2);
    let c = severity_event("c", 2, 3);
    let mut f = EventFollow::default();
    let mut view = GlobeViewState::from_focus(GeoPoint {
        lat: 10.0,
        lon: 20.0,
    });
    f.enable(&[a.clone(), b, c.clone()]);
    assert_eq!(f.tick(&mut view, 0.0).as_deref(), Some("factal-a"));
    f.observe(&[a, severity_event("b", 5, 2)]);
    assert_eq!(f.tick(&mut view, 1.0).as_deref(), Some("factal-b"));
    assert_eq!(f.tour_position(), Some((1, 2)));
    assert_eq!(finish_stop(&mut f, &mut view), "factal-a");
    assert_eq!(finish_stop(&mut f, &mut view), "factal-b");
}

#[test]
fn single_or_empty_payload_keeps_orbiting_without_restarting_a_flight() {
    let a = event("a", 10.0, 20.0, 1);
    let mut f = EventFollow::default();
    let mut view = GlobeViewState::from_focus(GeoPoint {
        lat: 10.0,
        lon: 20.0,
    });
    f.enable(std::slice::from_ref(&a));
    f.tick(&mut view, 0.0);
    for now in [25.0, 60.0, 200.0] {
        f.observe(std::slice::from_ref(&a));
        assert_eq!(f.tick(&mut view, now), None);
        assert_eq!(f.flight.as_ref().unwrap().started, 0.0);
    }
    f.observe(&[]);
    assert_eq!(f.tick(&mut view, 250.0), None);
    assert_eq!(f.tour_position(), None);
    assert_eq!(f.target().unwrap().id, "factal-a");
}

#[test]
fn automatic_next_stop_reopens_brief_without_snapping_or_changing_view_mode() {
    let mut model = AppModel::new();
    model.globe_view.local_mode = true;
    model.replace_factal_events(vec![severity_event("a", 4, 1), severity_event("b", 2, 2)]);
    model.toggle_event_follow();
    model.tick_event_follow(0.0);
    let flight = model.event_follow.flight.as_ref().unwrap();
    let deadline = flight.duration + ORBIT_SECONDS;
    let expected = flight.sample(deadline).0;
    model.factal_brief_open = false;
    model.tick_event_follow(deadline);
    assert_eq!(model.selected_event_id.as_deref(), Some("factal-b"));
    assert!(model.factal_brief_open);
    assert!(model.globe_view.local_mode);
    assert!((model.globe_view.local_center.lat as f64 - expected[0]).abs() < 1e-5);
    assert!((model.globe_view.local_yaw as f64 - expected[3]).abs() < 1e-5);
}
