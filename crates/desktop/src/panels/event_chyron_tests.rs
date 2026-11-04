use super::*;
use crate::model::{FactalBrief, GeoPoint};

fn event(index: usize) -> EventRecord {
    EventRecord {
        id: format!("factal-{index}"),
        title: format!("Story {index}"),
        summary: format!("Full summary for story {index}, including its final detail."),
        severity: if index == 0 {
            EventSeverity::Critical
        } else {
            EventSeverity::Elevated
        },
        location_name: "Fixture location".into(),
        location: GeoPoint { lat: 0.0, lon: 0.0 },
        source: "Factal".into(),
        occurred_at: format!("2026-10-31 12:{index:02}:00"),
        factal_brief: Some(FactalBrief {
            factal_id: index.to_string(),
            severity_value: Some(if index == 0 { 5 } else { 2 }),
            occurred_at_raw: None,
            point_wkt: None,
            vertical: None,
            subvertical: None,
            topics: vec![],
            content: None,
            raw_json_pretty: "{}".into(),
        }),
    }
}

fn frame(
    ctx: &egui::Context,
    events: &[EventRecord],
    selected: Option<&str>,
    width: f32,
    time: f64,
) -> egui::FullOutput {
    let viewport = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 720.0));
    ctx.run(
        egui::RawInput {
            screen_rect: Some(viewport),
            time: Some(time),
            ..Default::default()
        },
        |ctx| {
            draw(
                &ctx.layer_painter(egui::LayerId::background()),
                viewport,
                events.iter(),
                selected,
                std::iter::empty(),
            );
        },
    )
}

fn texts(
    output: &egui::FullOutput,
) -> Vec<(&egui::epaint::ClippedShape, &egui::epaint::TextShape)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                Some((shape, text))
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn recap_shows_only_reached_history_and_keeps_live_crawl_separate() {
    let ctx = egui::Context::default();
    frame(&ctx, &[event(99)], Some("factal-99"), 1100.0, 0.0);
    let history = (0..8)
        .map(|index| {
            let mut event = event(index);
            event.id = format!("hist-{index}");
            (index as i64 * 10, event)
        })
        .collect();
    let mut replay = crate::model::ReplayState::new(history, 0, 100, 60.0);
    replay.advance_to(30.0);
    let viewport = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 720.0));
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(viewport),
            time: Some(30.0),
            ..Default::default()
        },
        |ctx| {
            clear(ctx);
            draw_recap(
                &ctx.layer_painter(egui::LayerId::background()),
                viewport,
                &replay,
            );
        },
    );
    let lines: Vec<_> = texts(&output)
        .iter()
        .map(|(_, text)| text.galley.text().to_owned())
        .collect();
    assert_eq!(lines.len(), 5);
    for index in 1..=5 {
        assert!(
            lines
                .iter()
                .any(|text| text.starts_with(&format!("Story {index} -")))
        );
    }
    for index in [0, 6, 7, 99] {
        assert!(
            !lines
                .iter()
                .any(|text| text.starts_with(&format!("Story {index} -")))
        );
    }
    let live = frame(&ctx, &[event(88)], Some("factal-88"), 1100.0, 31.0);
    assert_eq!(texts(&live).len(), 1);
    assert!(texts(&live)[0].1.galley.text().starts_with("Story 88 -"));
}

#[test]
fn paints_five_single_lines_with_the_full_title_level_and_summary() {
    for width in [480.0, 1100.0, 1920.0] {
        let ctx = egui::Context::default();
        let events: Vec<_> = (0..8).map(event).collect();
        let output = frame(&ctx, &events, Some("factal-7"), width, 0.0);
        let lines = texts(&output);
        assert_eq!(lines.len(), 5);
        assert!(
            lines[0]
                .1
                .galley
                .text()
                .starts_with("Story 7 - Elevated - ")
        );
        for (shape, text) in &lines {
            assert_eq!(text.galley.rows.len(), 1);
            assert!(text.galley.text().ends_with("including its final detail."));
            assert!(shape.clip_rect.width() < width);
            assert!(shape.clip_rect.bottom() <= 720.0);
            assert_eq!(shape.clip_rect.height(), ROW_HEIGHT);
        }
        assert!(
            lines
                .windows(2)
                .all(|pair| pair[0].1.pos.y < pair[1].1.pos.y)
        );
        let ids = ctx.data(|data| {
            data.get_temp::<State>(state_id())
                .unwrap()
                .rows
                .iter()
                .flatten()
                .map(|row| row.story.id.clone())
                .collect::<HashSet<_>>()
        });
        assert_eq!(ids.len(), 5);
    }
}

#[test]
fn long_summary_scrolls_left_and_reaches_its_final_words_before_replacement() {
    let ctx = egui::Context::default();
    let mut events: Vec<_> = (0..8).map(event).collect();
    events[0].summary = "The full report keeps every word readable while the camera travels. "
        .repeat(12)
        + "FINAL WORDS";
    let first = frame(&ctx, &events, Some("factal-0"), 800.0, 0.0);
    let first_text = texts(&first)[0].1;
    let origin = first_text.pos.x;
    let width = first_text.galley.size().x;
    let clip = texts(&first)[0].0.clip_rect;
    let hold = frame(&ctx, &events, Some("factal-0"), 800.0, 1.0);
    assert_eq!(texts(&hold)[0].1.pos.x, origin);
    let moving = frame(&ctx, &events, Some("factal-0"), 800.0, 3.0);
    assert!((texts(&moving)[0].1.pos.x - (origin - SCROLL_SPEED as f32)).abs() < 0.1);
    let finish = START_HOLD + (width - clip.width()) as f64 / SCROLL_SPEED + 0.2;
    let ending = frame(&ctx, &events, Some("factal-0"), 800.0, finish);
    let ending = texts(&ending)
        .into_iter()
        .find(|(_, text)| text.galley.text().ends_with("FINAL WORDS"))
        .unwrap();
    assert!((ending.1.pos.x + width - ending.0.clip_rect.right()).abs() < 0.1);
}

#[test]
fn camera_and_poll_changes_do_not_restart_or_replace_unfinished_rows() {
    let ctx = egui::Context::default();
    let mut events: Vec<_> = (0..12).map(event).collect();
    for event in &mut events {
        event.summary = event.summary.repeat(30);
    }
    let first = frame(&ctx, &events, Some("factal-0"), 800.0, 0.0);
    let original: Vec<_> = texts(&first)
        .iter()
        .map(|(_, text)| text.galley.text().to_owned())
        .collect();
    for event in &mut events {
        event.title = "Revised in poll".into();
    }
    events.reverse();
    let next = frame(&ctx, &events, Some("factal-10"), 800.0, 3.0);
    let displayed: Vec<_> = texts(&next)
        .iter()
        .map(|(_, text)| text.galley.text().to_owned())
        .collect();
    assert_eq!(original, displayed);
    let state = ctx.data(|data| data.get_temp::<State>(state_id()).unwrap());
    assert!(state.rows.iter().flatten().all(|row| row.started == 0.0));
}

#[test]
fn independent_rows_rotate_after_reading_and_pick_up_the_new_selection() {
    let ctx = egui::Context::default();
    let mut events: Vec<_> = (0..12).map(event).collect();
    events[0].summary = events[0].summary.repeat(30);
    frame(&ctx, &events, Some("factal-0"), 800.0, 0.0);
    let output = frame(&ctx, &events, Some("factal-3"), 800.0, 11.0);
    let text = texts(&output);
    assert!(text[0].1.galley.text().starts_with("Story 0 - Critical - "));
    assert!(
        text.iter()
            .any(|(_, text)| text.galley.text().starts_with("Story 3 - Elevated - "))
    );
    let state = ctx.data(|data| data.get_temp::<State>(state_id()).unwrap());
    assert_eq!(state.rows[0].as_ref().unwrap().started, 0.0);
    assert_eq!(state.rows[1].as_ref().unwrap().started, 11.0);
    assert_eq!(
        state
            .rows
            .iter()
            .flatten()
            .map(|row| &row.story.id)
            .collect::<HashSet<_>>()
            .len(),
        5
    );
}

#[test]
fn empty_small_duplicate_and_non_factal_feeds_do_not_invent_stories() {
    let ctx = egui::Context::default();
    assert!(frame(&ctx, &[], None, 800.0, 0.0).shapes.is_empty());
    let mut duplicate = event(0);
    duplicate.id = "factal-duplicate".into();
    let mut quake = event(1);
    quake.id = "usgs-1".into();
    let output = frame(
        &ctx,
        &[event(0), duplicate, quake],
        Some("factal-0"),
        800.0,
        1.0,
    );
    assert_eq!(texts(&output).len(), 1);
    clear(&ctx);
    let output = frame(&ctx, &[event(1), event(2)], Some("factal-1"), 800.0, 100.0);
    assert_eq!(texts(&output).len(), 2);
    let state = ctx.data(|data| data.get_temp::<State>(state_id()).unwrap());
    assert!(state.rows.iter().flatten().all(|row| row.started == 100.0));
}

#[test]
fn visible_stories_fill_after_the_current_event_before_offscreen_stories() {
    let ctx = egui::Context::default();
    let events: Vec<_> = (0..12).map(event).collect();
    let viewport = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 720.0));
    let visible = ["factal-1", "factal-2", "factal-3", "factal-4"];
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(viewport),
            ..Default::default()
        },
        |ctx| {
            draw(
                &ctx.layer_painter(egui::LayerId::background()),
                viewport,
                events.iter(),
                Some("factal-0"),
                visible.into_iter(),
            );
        },
    );
    let state = ctx.data(|data| data.get_temp::<State>(state_id()).unwrap());
    let ids: HashSet<_> = state
        .rows
        .iter()
        .flatten()
        .map(|row| row.story.id.as_str())
        .collect();
    assert_eq!(
        ids,
        HashSet::from(["factal-0", "factal-1", "factal-2", "factal-3", "factal-4"])
    );
}
