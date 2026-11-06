//! Documented GWES station and live-stream APIs, joined by transmitter call sign.
use super::{RadioFeed, RadioKind, catalog::get_json, stream_url, valid_coordinates};
use reqwest::blocking::Client;
use serde_json::Value;

pub(super) fn fetch(client: &Client) -> Result<Vec<RadioFeed>, String> {
    let transmitters = get_json(
        client,
        reqwest::Url::parse("https://transmitters.weatherradio.org/").unwrap(),
    )?;
    let streams = get_json(
        client,
        reqwest::Url::parse("https://icestats.weatherradio.org/").unwrap(),
    )?;
    parse(&transmitters, &streams)
}

fn parse(transmitters: &Value, streams: &Value) -> Result<Vec<RadioFeed>, String> {
    let transmitters = transmitters
        .get("transmitters")
        .and_then(Value::as_object)
        .ok_or("WeatherRadio returned an invalid transmitter catalog.")?;
    let source = streams
        .pointer("/icestats/source")
        .ok_or("WeatherRadio returned no live stream list.")?;
    let entries: Vec<_> = match source {
        Value::Array(entries) => entries.iter().take(500).collect(),
        Value::Object(_) => vec![source],
        Value::Null => Vec::new(),
        _ => return Err("WeatherRadio returned an invalid stream list.".into()),
    };
    let mut feeds = Vec::new();
    for entry in entries {
        let Some(url) = entry
            .get("listenurl")
            .and_then(Value::as_str)
            .and_then(|u| stream_url(u).ok())
        else {
            continue;
        };
        let Some(mount) = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
        else {
            continue;
        };
        let callsign = mount
            .split('-')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let Some(station) = transmitters.get(&callsign) else {
            continue;
        };
        let coordinate = |key| {
            station
                .get(key)
                .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
        };
        let (latitude, longitude) = match (coordinate("LAT"), coordinate("LON")) {
            (Some(lat), Some(lon)) if valid_coordinates(lat, lon) => (Some(lat), Some(lon)),
            _ => (None, None),
        };
        let place = station
            .get("SITENAME")
            .and_then(Value::as_str)
            .unwrap_or(&callsign);
        let state = station
            .get("ST")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let provider = entry
            .get("server_description")
            .and_then(Value::as_str)
            .unwrap_or("Community-provided weather radio");
        feeds.push(RadioFeed {
            id: format!("gwes:{mount}"),
            name: format!("NOAA {callsign} · {place}"),
            url: url.to_string(),
            homepage: "https://www.weatherradio.org/".into(),
            kinds: vec![RadioKind::Weather],
            latitude,
            longitude,
            locality: format!("{state}, US · transmitter location · {provider}"),
            directory_id: None,
        });
    }
    Ok(feeds)
}

#[test]
fn weather_streams_join_transmitter_locations_and_keep_alternate_streams() {
    let transmitters = serde_json::json!({"transmitters": {"KDO89": {"LAT":"38.6", "LON":"-90.3", "SITENAME":"St. Louis", "ST":"MO"}}});
    let streams = serde_json::json!({"icestats":{"source":[
        {"listenurl":"https://wxr.gwes-cdn.net/KDO89", "server_description":"Stream provided by fixture"},
        {"listenurl":"https://wxr.gwes-cdn.net/KDO89-Alt2"}, {"listenurl":"https://wxr.gwes-cdn.net/UNKNOWN"}
    ]}});
    let feeds = parse(&transmitters, &streams).unwrap();
    assert_eq!(feeds.len(), 2);
    assert_eq!(feeds[1].latitude, Some(38.6));
    assert_ne!(feeds[0].id, feeds[1].id);
    assert_eq!(feeds[0].kinds, [RadioKind::Weather]);
    assert!(feeds[0].locality.contains("provided by fixture"));
    assert!(parse(&serde_json::json!({}), &streams).is_err());
}
