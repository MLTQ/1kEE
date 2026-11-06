//! Provider-published streams with explicitly approximate coverage locations.
use super::{RadioFeed, stream_url, valid_coordinates};
use serde::Deserialize;

#[derive(Deserialize)]
struct Snapshot {
    feeds: Vec<RadioFeed>,
}

pub(super) fn feeds() -> Vec<RadioFeed> {
    let snapshot: Snapshot =
        serde_json::from_str(include_str!("community_feeds.json")).expect("bundled radio snapshot");
    snapshot.feeds.into_iter().filter_map(|mut feed| {
        stream_url(&feed.url).ok()?;
        if !matches!((feed.latitude, feed.longitude), (Some(lat), Some(lon)) if valid_coordinates(lat, lon)) {
            feed.latitude = None; feed.longitude = None;
        }
        Some(feed)
    }).collect()
}

#[test]
fn independent_snapshot_has_published_urls_and_explicit_coverage_centers() {
    let snapshot: Snapshot = serde_json::from_str(include_str!("community_feeds.json")).unwrap();
    assert_eq!(
        snapshot
            .feeds
            .iter()
            .filter(|e| e.id.starts_with("nciss:"))
            .count(),
        18
    );
    assert_eq!(
        snapshot
            .feeds
            .iter()
            .filter(|e| e.id.starts_with("scanbc:"))
            .count(),
        22
    );
    assert_eq!(feeds().iter().filter(|e| e.latitude.is_some()).count(), 34);
    for feed in snapshot.feeds {
        assert!(stream_url(&feed.url).is_ok());
        assert!(!feed.kinds.is_empty());
        assert!(!feed.locality.is_empty());
        assert!(feed.directory_id.is_none());
    }
}
