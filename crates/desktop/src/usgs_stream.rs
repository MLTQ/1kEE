use crate::model::{AppModel, EventRecord, EventSeverity, FactalBrief, GeoPoint, USGS_EVENT_PREFIX};
use reqwest::blocking::Client;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const USGS_FEED_URL: &str =
    "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/2.5_day.geojson";
/// The feed refreshes roughly every minute, but M2.5+ quakes are not that
/// frequent — poll gently to be a good citizen of the public endpoint.
const POLL_INTERVAL: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

struct PollManager {
    active: Option<JoinHandle<PollOutcome>>,
    next_poll_at: Option<Instant>,
    shutdown: bool,
}

enum PollOutcome {
    Success(Vec<EventRecord>),
    RateLimited,
    Error(String),
}

/// Advance the USGS earthquake polling lifecycle. Called every frame from
/// `app.rs`; cheap unless a poll result is ready to apply. Requires no
/// configuration — the feed is public and keyless.
pub fn tick(model: &mut AppModel) {
    let now = Instant::now();
    let mut finished = None;

    {
        let mut manager = manager().lock().unwrap();
        if manager.active.as_ref().is_some_and(|h| h.is_finished()) {
            finished = manager.active.take();
        }
    }

    if let Some(handle) = finished {
        match handle.join() {
            Ok(outcome) => apply_outcome(model, outcome),
            Err(_) => {
                model.usgs_stream_status = "error".into();
                model.push_log("USGS quake poll worker panicked before returning data.".into());
            }
        }
    }

    let mut should_spawn = false;
    {
        let mut manager = manager().lock().unwrap();
        if !manager.shutdown
            && manager.active.is_none()
            && now >= manager.next_poll_at.unwrap_or(now)
        {
            should_spawn = true;
            manager.next_poll_at = Some(now + POLL_INTERVAL);
        }
    }

    if should_spawn {
        let handle = thread::spawn(|| {
            let outcome = fetch_latest_quakes();
            // Wake the UI so the result is applied promptly even when idle.
            crate::app::request_repaint();
            outcome
        });

        let mut manager = manager().lock().unwrap();
        if !manager.shutdown {
            manager.active = Some(handle);
            if model.usgs_stream_status != "live" {
                model.usgs_stream_status = "syncing".into();
            }
        }
    }
}

/// Stop launching new USGS polls during app teardown.
pub fn shutdown() {
    let mut manager = manager().lock().unwrap();
    manager.shutdown = true;
    manager.next_poll_at = None;
}

fn apply_outcome(model: &mut AppModel, outcome: PollOutcome) {
    match outcome {
        PollOutcome::Success(events) => {
            let count = events.len();
            let previous = model
                .events
                .iter()
                .filter(|event| event.id.starts_with(USGS_EVENT_PREFIX))
                .count();
            let should_log = model.usgs_stream_status != "live" || previous != count;
            // Persist to history store before merging into live events.
            crate::event_store::upsert_events(&events);
            model.replace_usgs_events(events);
            model.usgs_stream_status = "live".into();
            if should_log {
                model.push_log(format!(
                    "USGS sync loaded {} M2.5+ earthquake(s) from the past day.",
                    count
                ));
            }
        }
        PollOutcome::RateLimited => {
            let was_status = model.usgs_stream_status.clone();
            model.usgs_stream_status = "rate limited".into();
            if was_status != "rate limited" {
                model.push_log(
                    "USGS feed rate limited the quake poll; the app will retry automatically."
                        .into(),
                );
            }
        }
        PollOutcome::Error(error) => {
            model.usgs_stream_status = "error".into();
            model.push_log(format!("USGS quake poll failed: {}", error));
        }
    }
}

fn manager() -> &'static Mutex<PollManager> {
    static MANAGER: OnceLock<Mutex<PollManager>> = OnceLock::new();
    MANAGER.get_or_init(|| {
        Mutex::new(PollManager {
            active: None,
            next_poll_at: Some(Instant::now()),
            shutdown: false,
        })
    })
}

fn fetch_latest_quakes() -> PollOutcome {
    let client = match Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => return PollOutcome::Error(error.to_string()),
    };

    let response = match client.get(USGS_FEED_URL).send() {
        Ok(response) => response,
        Err(error) => return PollOutcome::Error(error.to_string()),
    };

    match response.status().as_u16() {
        200 => {}
        429 => return PollOutcome::RateLimited,
        code => {
            return PollOutcome::Error(format!("USGS feed returned unexpected status {}", code));
        }
    }

    let body = match response.text() {
        Ok(body) => body,
        Err(error) => return PollOutcome::Error(error.to_string()),
    };

    match parse_feed(&body) {
        Ok(events) => PollOutcome::Success(events),
        Err(error) => PollOutcome::Error(error),
    }
}

/// Parse a USGS GeoJSON summary feed body into normalized `EventRecord`s.
fn parse_feed(body: &str) -> Result<Vec<EventRecord>, String> {
    let payload: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;

    let mut ids = HashSet::new();
    let mut events = Vec::new();
    for raw_feature in payload
        .get("features")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(event) = parse_feature(raw_feature) {
            if ids.insert(event.id.clone()) {
                events.push(event);
            }
        }
    }

    Ok(events)
}

fn parse_feature(raw: &Value) -> Option<EventRecord> {
    let feature_id = raw
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_owned();
    let props = raw.get("properties")?;

    // GeoJSON coordinate order is [lon, lat, depth_km] — longitude first.
    let coordinates = raw
        .get("geometry")
        .and_then(|geometry| geometry.get("coordinates"))
        .and_then(Value::as_array)?;
    let lon = value_as_f64(coordinates.first())?;
    let lat = value_as_f64(coordinates.get(1))?;
    let depth_km = value_as_f64(coordinates.get(2));

    let magnitude = value_as_f64(props.get("mag"));
    let place = props
        .get("place")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let tsunami = props.get("tsunami").and_then(Value::as_i64).unwrap_or(0) != 0;

    let time_ms = props.get("time").and_then(Value::as_i64)?;
    let occurred_unix = time_ms.div_euclid(1000);
    let datetime = crate::event_store::unix_to_datetime_str(occurred_unix);
    let occurred_at = format!("{datetime} UTC");
    let occurred_at_raw = format!("{}Z", datetime.replacen(' ', "T", 1));

    let severity = quake_severity(magnitude);
    let title = props
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| match magnitude {
            Some(magnitude) => format!("M {:.1} earthquake", magnitude),
            None => "Earthquake reported by USGS".into(),
        });

    let mut summary = match magnitude {
        Some(magnitude) => format!("Magnitude {:.1} earthquake", magnitude),
        None => "Earthquake of unreported magnitude".to_owned(),
    };
    if let Some(depth_km) = depth_km {
        summary.push_str(&format!(" at {:.0} km depth", depth_km));
    }
    match &place {
        Some(place) => summary.push_str(&format!(", {}.", place)),
        None => summary.push('.'),
    }
    if tsunami {
        summary.push_str(" USGS has flagged a tsunami advisory for this event.");
    }

    let location = GeoPoint {
        lat: lat as f32,
        lon: lon as f32,
    };
    let location_name = place.unwrap_or_else(|| format!("{:.4}, {:.4}", lat, lon));
    let severity_value = match severity {
        EventSeverity::Critical => 4,
        EventSeverity::Elevated => 2,
        EventSeverity::Advisory => 1,
    };
    let id = format!("{USGS_EVENT_PREFIX}{feature_id}");

    Some(EventRecord {
        id: id.clone(),
        title,
        summary,
        severity,
        location_name,
        location,
        source: "USGS".into(),
        occurred_at,
        // The brief doubles as the event-store persistence payload
        // (`event_store::upsert_events` skips records without one), so quakes
        // carry one even though they are not Factal items. `factal_id` keeps
        // the `usgs-` prefix so the store's primary key cannot collide with
        // real Factal ids.
        factal_brief: Some(FactalBrief {
            factal_id: id,
            severity_value: Some(severity_value),
            occurred_at_raw: Some(occurred_at_raw),
            point_wkt: None,
            vertical: Some("Earthquake".into()),
            subvertical: None,
            topics: vec!["Earthquake".into()],
            content: None,
            raw_json_pretty: serde_json::to_string_pretty(raw).unwrap_or_else(|_| raw.to_string()),
        }),
    })
}

fn quake_severity(magnitude: Option<f64>) -> EventSeverity {
    match magnitude {
        Some(magnitude) if magnitude >= 6.0 => EventSeverity::Critical,
        Some(magnitude) if magnitude >= 4.5 => EventSeverity::Elevated,
        _ => EventSeverity::Advisory,
    }
}

fn value_as_f64(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    if let Some(number) = value.as_f64() {
        Some(number)
    } else if let Some(number) = value.as_i64() {
        Some(number as f64)
    } else if let Some(text) = value.as_str() {
        text.parse::<f64>().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_FEED: &str = r#"{
        "type": "FeatureCollection",
        "metadata": {"title": "USGS Magnitude 2.5+ Earthquakes, Past Day"},
        "features": [
            {
                "type": "Feature",
                "properties": {
                    "mag": 6.5,
                    "place": "120 km SSW of Severo-Kuril'sk, Russia",
                    "time": 1719900000000,
                    "tsunami": 1,
                    "title": "M 6.5 - 120 km SSW of Severo-Kuril'sk, Russia"
                },
                "geometry": {"type": "Point", "coordinates": [155.201, 49.437, 35.0]},
                "id": "us7000quake1"
            },
            {
                "type": "Feature",
                "properties": {
                    "mag": 4.5,
                    "place": "10 km SSW of Guánica, Puerto Rico",
                    "time": 1719903600000,
                    "tsunami": 0,
                    "title": "M 4.5 - 10 km SSW of Guánica, Puerto Rico"
                },
                "geometry": {"type": "Point", "coordinates": [-66.941, 17.887, 10.2]},
                "id": "us7000quake2"
            },
            {
                "type": "Feature",
                "properties": {
                    "mag": 2.8,
                    "place": "5 km NE of Ridgecrest, CA",
                    "time": 1719907200000,
                    "tsunami": 0,
                    "title": "M 2.8 - 5 km NE of Ridgecrest, CA"
                },
                "geometry": {"type": "Point", "coordinates": [-117.634, 35.655, 4.1]},
                "id": "ci40000quake3"
            }
        ]
    }"#;

    #[test]
    fn parses_usgs_feed_features() {
        let events = parse_feed(SAMPLE_FEED).expect("sample feed should parse");
        assert_eq!(events.len(), 3);

        let critical = &events[0];
        assert_eq!(critical.id, "usgs-us7000quake1");
        assert_eq!(critical.title, "M 6.5 - 120 km SSW of Severo-Kuril'sk, Russia");
        assert_eq!(critical.severity, EventSeverity::Critical);
        // GeoJSON order is [lon, lat]; make sure it was not swapped.
        assert!((critical.location.lat - 49.437).abs() < 1e-3);
        assert!((critical.location.lon - 155.201).abs() < 1e-3);
        assert!(critical.summary.contains("tsunami"));
        assert_eq!(critical.source, "USGS");
        assert_eq!(critical.occurred_at, "2024-07-02 06:00:00 UTC");

        let brief = critical.factal_brief.as_ref().expect("persistence brief");
        assert_eq!(brief.factal_id, "usgs-us7000quake1");
        assert_eq!(
            brief.occurred_at_raw.as_deref(),
            Some("2024-07-02T06:00:00Z")
        );
        assert_eq!(
            crate::event_store::parse_iso_to_unix("2024-07-02T06:00:00Z"),
            Some(1719900000)
        );

        assert_eq!(events[1].severity, EventSeverity::Elevated);
        assert_eq!(events[2].severity, EventSeverity::Advisory);
        assert_eq!(events[2].summary, "Magnitude 2.8 earthquake at 4 km depth, 5 km NE of Ridgecrest, CA.");
    }

    #[test]
    fn severity_mapping_boundaries() {
        assert_eq!(quake_severity(Some(6.0)), EventSeverity::Critical);
        assert_eq!(quake_severity(Some(5.9)), EventSeverity::Elevated);
        assert_eq!(quake_severity(Some(4.5)), EventSeverity::Elevated);
        assert_eq!(quake_severity(Some(4.4)), EventSeverity::Advisory);
        assert_eq!(quake_severity(None), EventSeverity::Advisory);
    }
}
