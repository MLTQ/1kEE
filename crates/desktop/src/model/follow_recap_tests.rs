use super::*;
use crate::model::{EventSeverity, FactalBrief};

fn event(id: &str, point: GeoPoint) -> EventRecord {
    EventRecord {
        id: format!("factal-{id}"),
        title: format!("Report {id}"),
        summary: format!("Summary {id}"),
        severity: EventSeverity::Elevated,
        location_name: id.into(),
        location: point,
        source: "Fixture".into(),
        occurred_at: "2026-11-04".into(),
        factal_brief: Some(FactalBrief {
            factal_id: id.into(),
            severity_value: Some(2),
            occurred_at_raw: None,
            point_wkt: None,
            vertical: None,
            subvertical: None,
            topics: vec![],
            content: None,
            raw_json_pretty: String::new(),
        }),
    }
}

fn model(local: bool) -> AppModel {
    let mut model = AppModel::new();
    model.factal_api_key.clear();
    model.events = vec![event(
        "live",
        GeoPoint {
            lat: 40.0,
            lon: -74.0,
        },
    )];
    model.globe_view.local_mode = local;
    model.toggle_event_follow();
    model.tick_event_follow(0.0);
    model
}

/// Supply deterministic cached data in place of the asynchronous SQLite result.
fn loaded(model: &mut AppModel, events: History) {
    let recap = model.follow_recaps.active.as_mut().unwrap();
    recap.pending = None;
    recap.set_events(events);
}

#[test]
fn continents_cover_major_landmasses_islands_and_dateline_without_duplicate_assignments() {
    use Continent::*;
    for (lat, lon, expected) in [
        (40.7, -74.0, NorthAmerica),
        (19.4, -99.1, NorthAmerica),
        (9.0, -79.5, NorthAmerica),
        (18.5, -66.0, NorthAmerica),
        (64.0, -150.0, NorthAmerica),
        (52.0, 179.0, NorthAmerica),
        (52.0, -179.0, NorthAmerica),
        (1.8, -157.4, Oceania),
        (21.3, -157.8, NorthAmerica),
        (72.0, -42.0, NorthAmerica),
        (4.7, -74.1, SouthAmerica),
        (10.5, -66.9, SouthAmerica),
        (-34.6, -58.4, SouthAmerica),
        (-53.0, -70.9, SouthAmerica),
        (51.5, -0.1, Europe),
        (64.1, -21.9, Europe),
        (50.5, 30.5, Europe),
        (38.0, 23.7, Europe),
        (55.8, 37.6, Europe),
        (30.0, 31.2, Africa),
        (36.8, 10.2, Africa),
        (-1.3, 36.8, Africa),
        (-18.9, 47.5, Africa),
        (-33.9, 18.4, Africa),
        (14.9, -23.5, Africa),
        (24.7, 46.7, Asia),
        (15.4, 44.2, Asia),
        (39.9, 32.9, Asia),
        (35.7, 139.7, Asia),
        (28.6, 77.2, Asia),
        (35.9, 14.5, Europe),
        (-6.2, 106.8, Asia),
        (14.6, 121.0, Asia),
        (66.0, 179.0, Asia),
        (-33.9, 151.2, Oceania),
        (-36.8, 174.8, Oceania),
        (-9.4, 147.2, Oceania),
        (-18.1, 178.4, Oceania),
        (-13.8, -172.1, Oceania),
        (-17.6, -149.6, Oceania),
        (-78.0, 166.0, Antarctica),
    ] {
        assert_eq!(
            Continent::at(GeoPoint { lat, lon }),
            Some(expected),
            "{lat}, {lon}"
        );
    }
    for lat in -90..=90 {
        for lon in -180..=180 {
            assert!(
                Continent::at(GeoPoint {
                    lat: lat as f32,
                    lon: lon as f32
                })
                .is_some()
            );
        }
    }
    assert_eq!(
        Continent::at(GeoPoint {
            lat: -20.0,
            lon: 180.0
        }),
        Continent::at(GeoPoint {
            lat: -20.0,
            lon: -180.0
        })
    );
    for point in [
        GeoPoint {
            lat: f32::NAN,
            lon: 0.0,
        },
        GeoPoint {
            lat: 91.0,
            lon: 0.0,
        },
        GeoPoint {
            lat: 0.0,
            lon: 181.0,
        },
    ] {
        assert_eq!(Continent::at(point), None);
    }
}

#[test]
fn full_rotation_waits_five_live_minutes_and_resumes_in_both_views() {
    for local in [false, true] {
        let mut model = model(local);
        model.event_follow.set_dwell_seconds(90);
        let mut live_start = 0.0;
        for expected in Continent::ALL.into_iter().chain([Continent::NorthAmerica]) {
            model.tick_event_follow(live_start + LIVE_SECONDS - 0.01);
            assert!(model.follow_recaps.active.is_none());
            let started = live_start + LIVE_SECONDS;
            model.tick_event_follow(started);
            assert!(model.event_follow.enabled());
            assert_eq!(
                model.follow_recaps.active.as_ref().unwrap().continent,
                expected
            );
            assert!(!model.globe_view.local_mode);
            loaded(&mut model, vec![]);
            model.tick_event_follow(started + FRAMING_SECONDS);
            assert_eq!(
                model.follow_recaps.active.as_ref().unwrap().clock(),
                "00:00:00"
            );
            model.tick_event_follow(started + FRAMING_SECONDS + 30.0);
            assert_eq!(
                model.follow_recaps.active.as_ref().unwrap().clock(),
                "12:00:00"
            );
            model.tick_event_follow(started + FRAMING_SECONDS + RECAP_SECONDS);
            assert_eq!(
                model.follow_recaps.active.as_ref().unwrap().clock(),
                "23:59:59"
            );
            live_start = started + FRAMING_SECONDS + RECAP_SECONDS + 0.5;
            model.tick_event_follow(live_start);
            assert!(model.follow_recaps.active.is_none());
            assert!(model.event_follow.enabled());
            assert_eq!(model.globe_view.local_mode, local);
            assert_eq!(model.event_follow.dwell_seconds(), 90);
            assert!(model.factal_brief_open);
        }
    }
}

#[test]
fn snapshot_is_filtered_to_one_continent_and_exact_trailing_day() {
    let view = GlobeViewState::from_focus(GeoPoint {
        lat: 0.0,
        lon: 179.0,
    });
    let unix = 1_800_000_000;
    let na = GeoPoint {
        lat: 40.0,
        lon: -100.0,
    };
    let mut recap = ContinentalRecap::with_events(
        Continent::NorthAmerica,
        5.0,
        unix,
        view,
        vec![
            (unix, event("last", na)),
            (unix - DAY, event("first", na)),
            (unix - DAY - 1, event("old", na)),
            (unix + 1, event("future", na)),
            (unix - 100, event("africa", Continent::Africa.center())),
        ],
    );
    assert_eq!(recap.replay.events.len(), 2);
    assert_eq!(recap.replay.events[0].1.id, "factal-first");
    let mut view = view;
    recap.tick(&mut view, 8.0);
    assert_eq!(recap.replay.revealed_events().len(), 1);
    assert!((view.globe_center_latlon().lon + 100.0).abs() < 0.001);
    recap.tick(&mut view, 68.0);
    assert_eq!(recap.replay.revealed_events().len(), 2);
    recap.tick(&mut view, 68.25);
    let last = recap.replay.active_flares.last().unwrap();
    assert_eq!(last.event.id, "factal-last");
    assert!(last.alpha(recap.replay.wall_elapsed()) > 0.0);
}

#[test]
fn loading_waits_without_advancing_clock_and_skipped_frames_do_not_replay_old_flares() {
    let mut model = model(false);
    model.tick_event_follow(300.0);
    let (tx, rx) = mpsc::channel();
    model.follow_recaps.active.as_mut().unwrap().pending = Some(rx);
    model.tick_event_follow(320.0);
    assert!(model.follow_recaps.active.as_ref().unwrap().loading());
    assert_eq!(
        model.follow_recaps.active.as_ref().unwrap().clock(),
        "00:00:00"
    );
    let recap = model.follow_recaps.active.as_ref().unwrap();
    let start = recap.replay.sim_from;
    tx.send(vec![
        (start, event("start", Continent::NorthAmerica.center())),
        (
            start + DAY / 2,
            event("middle", Continent::NorthAmerica.center()),
        ),
    ])
    .unwrap();
    model.tick_event_follow(321.0);
    assert_eq!(model.active_replay().unwrap().revealed_events().len(), 1);
    model.tick_event_follow(352.0);
    let replay = model.active_replay().unwrap();
    assert_eq!(replay.revealed_events().len(), 2);
    assert_eq!(replay.active_flares.len(), 1);
    assert_eq!(replay.active_flares[0].event.id, "factal-middle");
    model.tick_event_follow(1000.0);
    assert!(model.follow_recaps.active.is_none());
    model.tick_event_follow(1001.0);
    assert!(model.follow_recaps.active.is_none());
}

#[test]
fn cancellation_and_live_refresh_never_resume_an_abandoned_recap() {
    for cancel in 0..4 {
        let mut model = model(true);
        model.tick_event_follow(300.0);
        loaded(&mut model, vec![]);
        model.replace_factal_events(vec![event(
            "new",
            GeoPoint {
                lat: 35.0,
                lon: 139.0,
            },
        )]);
        model.tick_event_follow(310.0);
        assert!(model.follow_recaps.active.is_some());
        match cancel {
            0 => model.toggle_event_follow(),
            1 => model.select_event("factal-new"),
            2 => {
                model.active_body = ActiveBody::Moon;
                model.tick_event_follow(311.0);
            }
            _ => model.toggle_replay(),
        }
        assert!(!model.event_follow.enabled());
        assert!(model.follow_recaps.active.is_none());
        model.tick_event_follow(1000.0);
        assert!(model.follow_recaps.active.is_none());
        assert!(!model.event_follow.enabled());
    }
}

#[test]
fn recap_does_not_change_manual_replay_settings_or_restart_on_feed_updates() {
    let mut model = model(false);
    let settings = (
        model.replay_from_unix,
        model.replay_to_unix,
        model.replay_duration_secs,
    );
    model.tick_event_follow(300.0);
    loaded(&mut model, vec![]);
    model.tick_event_follow(303.0);
    model.replace_factal_events(vec![event(
        "new",
        GeoPoint {
            lat: 30.0,
            lon: 30.0,
        },
    )]);
    model.tick_event_follow(333.0);
    assert_eq!(
        model.follow_recaps.active.as_ref().unwrap().clock(),
        "12:00:00"
    );
    assert_eq!(
        (
            model.replay_from_unix,
            model.replay_to_unix,
            model.replay_duration_secs
        ),
        settings
    );
    assert!(!model.replay_mode);
}

#[test]
fn recap_hides_live_sidebar_and_brief_without_changing_open_flags() {
    let ctx = egui::Context::default();
    let mut model = model(false);
    let mut sidebar = crate::panels::EventCameraSidebar::default();
    model.tick_event_follow(300.0);
    loaded(&mut model, vec![]);
    for obs in [false, true] {
        model.obs_view = obs;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                ..Default::default()
            },
            |ctx| {
                sidebar.render(ctx, &mut model);
                crate::panels::render_factal_brief(ctx, &mut model);
                assert!(ctx.available_rect().width() > 1200.0);
            },
        );
        assert!(output.shapes.is_empty());
        assert!(model.factal_brief_open);
    }
    model.tick_event_follow(303.0);
    model.tick_event_follow(363.5);
    let output = ctx.run(Default::default(), |ctx| {
        sidebar.render(ctx, &mut model);
        crate::panels::render_factal_brief(ctx, &mut model);
    });
    assert!(output.shapes.iter().any(|shape| match &shape.shape {
        egui::Shape::Text(text) => text.galley.text() == "NEAREST CAMERAS",
        _ => false,
    }));
}
