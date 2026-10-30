use super::super::super::{FactalBrief, GeoPoint};
use super::*;

fn event(id: usize, severity: i64) -> EventRecord {
    EventRecord {
        id: format!("factal-{id}"),
        title: format!("Story {id}"),
        summary: String::new(),
        severity: EventSeverity::Elevated,
        location_name: "Fixture location".into(),
        location: GeoPoint {
            lat: 10.0,
            lon: 20.0,
        },
        source: "Factal".into(),
        occurred_at: String::new(),
        factal_brief: Some(FactalBrief {
            factal_id: id.to_string(),
            severity_value: Some(severity),
            occurred_at_raw: Some("2026-10-30T12:00:00Z".into()),
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
fn entire_equal_priority_pool_is_visited_without_a_six_record_cap() {
    let events: Vec<_> = (0..40).map(|id| event(id, 2)).collect();
    let mut tour = EventTour::default();
    tour.refresh(&events, None);
    let mut current = tour.take_pending().unwrap();
    let mut visited = HashSet::from([current.id.clone()]);
    for _ in 1..events.len() {
        current = tour.next(Some(&current.id)).unwrap();
        assert!(
            visited.insert(current.id.clone()),
            "repeated before covering the pool"
        );
    }
    assert_eq!(visited.len(), 40);
    assert_eq!(tour.position(&current.id).unwrap().1, 40);
}

#[test]
fn severity_increases_frequency_without_starving_other_stories_or_quick_repeats() {
    let events: Vec<_> = (0..48).map(|id| event(id, 2 + id as i64 / 12)).collect();
    let mut tour = EventTour::default();
    tour.refresh(&events, None);
    let mut current = None;
    let mut last_seen = HashMap::new();
    let mut counts = HashMap::<String, usize>::new();
    let mut totals = [0; 4];
    for step in 0..4800 {
        let chosen = tour.next(current.as_deref()).unwrap();
        if let Some(previous) = last_seen.insert(chosen.id.clone(), step) {
            assert!(
                step - previous > MAX_RECENT_STOPS,
                "repeat too soon: {}",
                chosen.id
            );
        }
        *counts.entry(chosen.id.clone()).or_default() += 1;
        totals[(severity(&chosen) - 2) as usize] += 1;
        current = Some(chosen.id);
    }
    assert_eq!(counts.len(), events.len());
    assert!(
        totals.windows(2).all(|pair| pair[0] < pair[1]),
        "{totals:?}"
    );
    assert!(
        totals[3] >= totals[0] * 2,
        "severity should matter materially: {totals:?}"
    );
}

#[test]
fn a_full_sized_feed_covers_every_record_and_revisits_major_events_more_often() {
    let events: Vec<_> = (0..360)
        .map(|id| {
            event(
                id,
                match id {
                    0..300 => 2,
                    300..340 => 3,
                    340..355 => 4,
                    _ => 5,
                },
            )
        })
        .collect();
    let mut tour = EventTour::default();
    tour.refresh(&events, None);
    let mut current = None;
    let mut counts = HashMap::<String, usize>::new();
    for _ in 0..1800 {
        let chosen = tour.next(current.as_deref()).unwrap();
        *counts.entry(chosen.id.clone()).or_default() += 1;
        current = Some(chosen.id);
    }
    assert_eq!(counts.len(), 360);
    let low_max = (0..300)
        .map(|id| counts[&format!("factal-{id}")])
        .max()
        .unwrap();
    let high_min = (355..360)
        .map(|id| counts[&format!("factal-{id}")])
        .min()
        .unwrap();
    assert!(high_min > low_max * 2, "high {high_min}, low {low_max}");
}

#[test]
fn repeated_reordered_refreshes_preserve_coverage_and_repeat_history() {
    let events: Vec<_> = (0..24).map(|id| event(id, 2 + id as i64 % 4)).collect();
    let mut control = EventTour::default();
    let mut refreshed = EventTour::default();
    control.refresh(&events, None);
    refreshed.refresh(&events, None);
    let mut current = None;
    for step in 0..500 {
        let mut payload = events.clone();
        payload.rotate_left(step % events.len());
        for event in &mut payload {
            event.title = format!("Updated {}", event.id);
        }
        refreshed.refresh(&payload, current.as_deref());
        let expected = control.next(current.as_deref()).unwrap();
        let actual = refreshed.next(current.as_deref()).unwrap();
        assert_eq!(actual.id, expected.id, "refresh reset scheduling at {step}");
        assert!(actual.title.starts_with("Updated"));
        current = Some(expected.id);
    }
}

#[test]
fn pool_changes_drop_expired_records_and_adjust_cooldown_for_small_pools() {
    for size in 2..16 {
        let events: Vec<_> = (0..size).map(|id| event(id, 2 + id as i64 % 4)).collect();
        let mut tour = EventTour::default();
        tour.refresh(&events, None);
        let mut current = tour.take_pending().unwrap();
        for _ in 0..100 {
            let next = tour
                .next(Some(&current.id))
                .expect("small tour must keep moving");
            assert_ne!(next.id, current.id);
            current = next;
        }
        let remaining = events
            .iter()
            .find(|event| event.id != current.id)
            .unwrap()
            .clone();
        tour.refresh(std::slice::from_ref(&remaining), Some(&current.id));
        let next = tour.next(Some(&current.id)).unwrap();
        assert_eq!(next.id, remaining.id);
        assert!(tour.next(Some(&next.id)).is_none());
        assert!(tour.position(&current.id).is_none());
        tour.refresh(&[], Some(&next.id));
        assert!(tour.next(Some(&next.id)).is_none());
    }
}

#[test]
fn rankings_use_numeric_severity_fallback_recency_and_unique_real_factal_records() {
    let mut fallback = event(2, 2);
    fallback.severity = EventSeverity::Critical;
    fallback.factal_brief.as_mut().unwrap().severity_value = None;
    let older = event(1, 5);
    let mut newer = event(3, 5);
    newer.factal_brief.as_mut().unwrap().occurred_at_raw = Some("2026-10-30T12:01:00Z".into());
    let mut quake = event(4, 9);
    quake.id = "usgs-quake".into(); // USGS also carries a persistence brief.
    let mut invalid = event(5, 9);
    invalid.location.lon = f32::INFINITY;
    let ordered = ranked(&[older, newer.clone(), newer, fallback, quake, invalid]);
    assert_eq!(
        ordered
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        ["factal-3", "factal-1", "factal-2"]
    );
}
