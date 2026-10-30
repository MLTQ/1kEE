use super::*;
use api::FetchStats;
use serde_json::json;

fn event(id: usize) -> EventRecord {
    super::parse::parse_event(&json!({
        "id": id,
        "date": "2026-10-30T12:00:00Z",
        "content": format!("Event {id}"),
        "severity": 2,
        "topics": [{"topic": {"name": "Location", "latitude": 40.0, "longitude": -74.0}}]
    }))
    .unwrap()
}

fn snapshot(events: Vec<EventRecord>) -> Snapshot {
    let count = events.len();
    Snapshot {
        events,
        stats: FetchStats {
            pages: 2,
            items: count,
            events: count,
        },
    }
}

fn poll_manager(now: Instant) -> PollManager {
    PollManager {
        active: None,
        generation: 7,
        tracked_key: String::new(),
        next_poll_at: Some(now),
        shutdown: false,
    }
}

#[test]
fn unchanged_factal_with_earthquakes_does_not_repeat_sync_logs() {
    let mut model = AppModel::new();
    model.events = vec![event(1)];
    let mut quake = event(2);
    quake.id = "usgs-quake".into();
    model.events.push(quake);
    model.factal_stream_status = "live".into();
    let previous_logs = model.activity_log.clone();
    apply_snapshot(&mut model, snapshot(vec![event(1)]));
    assert_eq!(model.activity_log, previous_logs);
    assert_eq!(model.events.len(), 2);
    assert!(model.events.iter().any(|event| event.id == "usgs-quake"));
}

#[test]
fn same_count_with_different_ids_reports_additions_and_removals() {
    let mut model = AppModel::new();
    model.events = vec![event(1)];
    model.factal_stream_status = "live".into();
    apply_snapshot(&mut model, snapshot(vec![event(2)]));
    let message = model.activity_log.last().unwrap();
    assert!(message.contains("1 new, 0 updated, 1 removed"));
    assert!(message.contains("1 geolocated event(s) in the past 24h"));
    assert!(message.contains("across 2 page(s)"));
}

#[test]
fn revised_payload_is_reported_and_applied() {
    let mut model = AppModel::new();
    model.events = vec![event(1)];
    model.factal_stream_status = "live".into();
    let mut revised = event(1);
    revised.summary = "Updated report".into();
    revised.factal_brief.as_mut().unwrap().raw_json_pretty =
        "{\"content\":\"Updated report\"}".into();
    apply_snapshot(&mut model, snapshot(vec![revised]));
    assert!(
        model
            .activity_log
            .last()
            .unwrap()
            .contains("0 new, 1 updated, 0 removed")
    );
    assert_eq!(model.events[0].summary, "Updated report");
}

#[test]
fn first_empty_sync_and_recovery_are_logged_but_unchanged_empty_sync_is_silent() {
    let mut model = AppModel::new();
    model.events.clear();
    model.factal_stream_status = "syncing".into();
    apply_snapshot(&mut model, snapshot(vec![]));
    assert!(model.activity_log.last().unwrap().contains("0 geolocated"));
    let log_count = model.activity_log.len();
    apply_snapshot(&mut model, snapshot(vec![]));
    assert_eq!(model.activity_log.len(), log_count);
    model.factal_stream_status = "error".into();
    apply_snapshot(&mut model, snapshot(vec![]));
    assert_eq!(model.factal_stream_status, "live");
    assert_eq!(model.activity_log.len(), log_count + 1);
}

#[test]
fn errors_and_stale_generations_preserve_the_previous_complete_feed() {
    let mut model = AppModel::new();
    model.events = vec![event(1)];
    let generation = current_generation();
    apply_outcome(&mut model, generation, Err(FetchError::RateLimited));
    assert_eq!(model.events[0].id, "factal-1");
    assert_eq!(model.factal_stream_status, "rate limited");
    apply_outcome(
        &mut model,
        generation,
        Err(FetchError::Other("page 2 failed".into())),
    );
    assert_eq!(model.events[0].id, "factal-1");
    let log_count = model.activity_log.len();
    apply_outcome(
        &mut model,
        generation.wrapping_add(1),
        Ok(snapshot(vec![event(2)])),
    );
    assert_eq!(model.events[0].id, "factal-1");
    assert_eq!(model.activity_log.len(), log_count);
}

#[test]
fn slow_sync_waits_a_minute_after_completion() {
    let start = Instant::now();
    let mut manager = poll_manager(start);
    let completed = start + Duration::from_secs(90);
    manager.poll_finished(7, completed);
    assert!(!manager.poll_due(completed));
    assert!(!manager.poll_due(completed + Duration::from_secs(59)));
    assert!(manager.poll_due(completed + Duration::from_secs(60)));
}

#[test]
fn active_workers_and_shutdown_prevent_another_poll() {
    let now = Instant::now();
    let mut manager = poll_manager(now);
    manager.active = Some(ActivePoll {
        generation: 7,
        handle: thread::spawn(|| Err(FetchError::Auth)),
    });
    assert!(!manager.poll_due(now + Duration::from_secs(600)));
    manager
        .active
        .take()
        .unwrap()
        .handle
        .join()
        .unwrap()
        .err()
        .unwrap();
    manager.shutdown = true;
    assert!(!manager.poll_due(now));
}

#[test]
fn obsolete_worker_does_not_delay_a_manual_refresh() {
    let now = Instant::now();
    let mut manager = poll_manager(now);
    manager.poll_finished(6, now + Duration::from_secs(90));
    assert!(manager.poll_due(now));
}
