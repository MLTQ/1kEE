use super::*;
use serde_json::json;

fn item(id: usize) -> Value {
    json!({
        "id": id,
        "date": "2026-10-30T12:00:00Z",
        "content": format!("Event {id}"),
        "severity": 2,
        "topics": [{"topic": {"name": "Location", "latitude": 40.0, "longitude": -74.0}}]
    })
}

#[test]
fn loads_beyond_fifty_and_deduplicates_across_pages() {
    let mut calls = Vec::new();
    let mut events = Vec::new();
    let stats = walk_pages(
        events_url(1_793_275_200),
        |url| {
            calls.push(url.clone());
            Ok(if calls.len() == 1 {
                Page {
                    results: (0..50).map(item).collect(),
                    next: Some("?severity__gte=2&offset=50".into()),
                }
            } else {
                Page {
                    results: (49..76).map(item).collect(),
                    next: None,
                }
            })
        },
        |page| events.extend_from_slice(page),
    )
    .unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].query().unwrap().contains("offset=50"));
    assert_eq!((stats.pages, stats.items, stats.events), (2, 77, 76));
    assert_eq!(events.len(), 76);
    assert_eq!(events[75].id, "factal-75");
}

#[test]
fn skips_ungeolocated_items_without_stopping_pagination() {
    let mut page_number = 0;
    let stats = walk_pages(
        events_url(0),
        |_| {
            page_number += 1;
            Ok(if page_number == 1 {
                Page {
                    results: vec![json!({"id": 1, "topics": []})],
                    next: Some(format!("{BASE_URL}?offset=50")),
                }
            } else {
                Page {
                    results: vec![item(2)],
                    next: None,
                }
            })
        },
        |_| {},
    )
    .unwrap();
    assert_eq!((stats.pages, stats.items, stats.events), (2, 2, 1));
}

#[test]
fn rejects_repeated_page_urls() {
    let initial = events_url(0);
    let result = walk_pages(
        initial.clone(),
        |_| {
            Ok(Page {
                results: vec![item(1)],
                next: Some(initial.to_string()),
            })
        },
        |_| {},
    );
    assert!(matches!(result, Err(FetchError::Other(message)) if message.contains("repeated")));
}

#[test]
fn rejects_next_links_outside_the_authenticated_item_endpoint() {
    for next in [
        "https://example.com/api/v2/item/?offset=50",
        "http://www.factal.com/api/v2/item/?offset=50",
        "https://www.factal.com/other/",
        "https://user@www.factal.com/api/v2/item/?offset=50",
    ] {
        let mut calls = 0;
        let result = walk_pages(
            events_url(0),
            |_| {
                calls += 1;
                Ok(Page {
                    results: vec![],
                    next: Some(next.into()),
                })
            },
            |_| {},
        );
        assert!(matches!(result, Err(FetchError::Other(_))));
        assert_eq!(calls, 1);
    }
}

#[test]
fn later_page_failure_does_not_return_a_successful_snapshot() {
    let mut calls = 0;
    let result = walk_pages(
        events_url(0),
        |_| {
            calls += 1;
            if calls == 1 {
                Ok(Page {
                    results: vec![item(1)],
                    next: Some("?offset=50".into()),
                })
            } else {
                Err(FetchError::RateLimited)
            }
        },
        |_| {},
    );
    assert!(matches!(result, Err(FetchError::RateLimited)));
    assert_eq!(calls, 2);
}

#[test]
fn malformed_results_fail_instead_of_becoming_an_empty_feed() {
    for payload in [json!({"detail": "error"}), json!({"results": {}})] {
        assert!(serde_json::from_value::<Page>(payload).is_err());
    }
    assert!(serde_json::from_value::<Page>(json!({"results": [], "next": null})).is_ok());
}

#[test]
fn query_uses_severity_and_an_exact_utc_cutoff() {
    let query: std::collections::HashMap<_, _> = events_url(86_400)
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(query["severity__gte"], "2");
    assert_eq!(query["date__gte"], "1970-01-02T00:00:00Z");
}

#[test]
#[ignore = "requires the local Factal token and makes a read-only live API sync"]
fn live_api_loads_complete_time_window() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let key =
        std::fs::read_to_string(root.join(".1kee_factal_api_key")).expect("local Factal token");
    let snapshot = fetch_live_events(key.trim()).expect("complete live sync");
    println!(
        "Factal live API: {} geolocated events, {} raw items across {} pages",
        snapshot.events.len(),
        snapshot.stats.items,
        snapshot.stats.pages
    );
    assert_eq!(snapshot.events.len(), snapshot.stats.events);
    let unique: HashSet<_> = snapshot.events.iter().map(|event| &event.id).collect();
    assert_eq!(unique.len(), snapshot.events.len());
    let earliest = crate::event_store::now_unix() - LIVE_WINDOW_SECS - 120;
    assert!(snapshot.events.iter().all(|event| {
        event
            .factal_brief
            .as_ref()
            .and_then(|brief| brief.occurred_at_raw.as_deref())
            .and_then(crate::event_store::parse_iso_to_unix)
            .is_some_and(|time| time >= earliest)
    }));
}
