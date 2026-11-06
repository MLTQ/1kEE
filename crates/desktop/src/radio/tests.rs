use super::*;

fn area(lat: f32, lon: f32) -> RadioArea {
    RadioArea {
        event_id: "test".into(),
        title: "Fixture incident".into(),
        name: "Fixture area".into(),
        location: GeoPoint { lat, lon },
    }
}
fn feed() -> RadioFeed {
    RadioFeed {
        id: "saved:test".into(),
        name: "Fixture fire dispatch".into(),
        url: "https://radio.example.org/live.mp3".into(),
        homepage: String::new(),
        kinds: vec![RadioKind::Fire],
        latitude: Some(0.0),
        longitude: Some(-179.9),
        locality: "Fixture".into(),
        directory_id: None,
    }
}

#[test]
fn distance_crosses_antimeridian_and_unknown_coordinates_are_not_invented() {
    let mut feed = feed();
    let distance = feed.distance_km(&area(0.0, 179.9)).unwrap();
    assert!((22.0..23.0).contains(&distance));
    feed.longitude = None;
    assert_eq!(feed.distance_km(&area(0.0, 0.0)), None);
    feed.longitude = Some(f64::NAN);
    assert_eq!(feed.distance_km(&area(0.0, 0.0)), None);
}

#[test]
fn directory_parser_preserves_provenance_and_excludes_music_and_restricted_audio() {
    let feeds = catalog::parse_fixture(
        r#"[
      {"stationuuid":"fire","name":"Kamloops Fire Rescue Scanner","url_resolved":"https://radio.example.org/fire.mp3","tags":"","lastcheckok":1,"geo_lat":50.67,"geo_long":-120.34,"state":"British Columbia","countrycode":"CA"},
      {"stationuuid":"unknown","name":"Fire dispatch","url":"https://radio.example.org/unknown","tags":"fire","lastcheckok":1},
      {"stationuuid":"music","name":"Exclusively The Police","url":"https://radio.example.org/music","tags":"exclusive,the police","lastcheckok":1},
      {"stationuuid":"music2","name":"scannerFM","url":"https://radio.example.org/music2","tags":"pop music","lastcheckok":1},
      {"stationuuid":"restricted","name":"Police dispatch","url":"https://audio1.broadcastify.com/123","lastcheckok":1},
      {"stationuuid":"offline","name":"Fire dispatch","url":"https://radio.example.org/offline","lastcheckok":0}
    ]"#,
    );
    assert_eq!(feeds.len(), 2);
    assert_eq!(feeds[0].directory_id.as_deref(), Some("fire"));
    assert_eq!(feeds[0].latitude, Some(50.67));
    assert_eq!(feeds[0].locality, "British Columbia, CA");
    assert!(feeds[0].kinds.contains(&RadioKind::Fire));
    assert_eq!(feeds[1].distance_km(&area(50.67, -120.34)), None);
    assert!(!catalog::classify("Film themes", "film themes").contains(&RadioKind::Ems));
}

#[test]
fn stream_validation_rejects_local_files_credentials_and_unapproved_providers() {
    for value in [
        "file:///etc/passwd",
        "https://user:secret@radio.example.org/live",
        "http://localhost/a",
        "http://127.0.0.1/live",
        "http://10.0.0.1/a",
        "http://[::ffff:127.0.0.1]/a",
        "http://[::1]/a",
        "http://100.64.0.1/a",
        "https://api.openmhz.com/call",
        "https://audio.radioreference.com/live",
        "https://audio1.broadcastify.com/live",
    ] {
        assert!(stream_url(value).is_err(), "accepted {value}");
    }
    assert!(stream_url("https://radio.example.org:8443/live.mp3").is_ok());
    assert!(stream_url("https://notbroadcastify.com/live").is_ok());
}

#[test]
fn saved_stream_round_trip_preserves_user_pin_and_rejects_corrupt_input() {
    let path = std::env::temp_dir().join(format!("1kee-radio-test-{}.json", std::process::id()));
    let original = feed();
    saved::save(&path, std::slice::from_ref(&original)).unwrap();
    let restored = saved::load(&path).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].url, original.url);
    assert_eq!(restored[0].longitude, original.longitude);
    std::fs::write(&path, "{broken").unwrap();
    assert!(saved::load(&path).is_err());
    let _ = std::fs::remove_file(path);
}

#[test]
#[ignore = "contacts public radio metadata APIs; explicit smoke test only"]
fn radio_live_catalog_smoke() {
    let report = catalog::fetch(&std::sync::atomic::AtomicBool::new(false)).unwrap();
    assert!(!report.feeds.is_empty());
    println!(
        "{} open feeds, {} with coordinates; warning: {:?}",
        report.feeds.len(),
        report.feeds.iter().filter(|f| f.latitude.is_some()).count(),
        report.warning
    );
    let mut sources = std::collections::BTreeMap::new();
    for feed in &report.feeds {
        *sources.entry(feed.source_label()).or_insert(0usize) += 1;
    }
    println!("Sources: {sources:?}");
    let weather = report
        .feeds
        .iter()
        .filter(|feed| feed.kinds == [RadioKind::Weather])
        .count();
    println!(
        "Categories: {weather} weather-only, {} other scanner/dispatch feeds",
        report.feeds.len() - weather
    );
}
