use crate::model::{AppModel, EventRecord, EventSeverity, FactalBrief, GeoPoint};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const FACTAL_LATEST_URL: &str = "https://www.factal.com/api/v2/item/?severity__gte=2";
const POLL_INTERVAL: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

struct PollManager {
    active: Option<ActivePoll>,
    generation: u64,
    tracked_key: String,
    next_poll_at: Option<Instant>,
    shutdown: bool,
}

struct ActivePoll {
    generation: u64,
    handle: JoinHandle<PollOutcome>,
}

enum PollOutcome {
    Success(Vec<EventRecord>),
    AuthError,
    RateLimited,
    Error(String),
}

pub fn tick(model: &mut AppModel) {
    let key = model.factal_api_key.trim().to_owned();
    let now = Instant::now();
    let mut finished = None;

    {
        let mut manager = manager().lock().unwrap();

        if manager.tracked_key != key {
            manager.tracked_key = key.clone();
            manager.generation = manager.generation.wrapping_add(1);
            manager.next_poll_at = Some(now);
            manager.shutdown = false;
        }

        if let Some(active) = manager.active.as_ref() {
            if active.handle.is_finished() {
                finished = manager
                    .active
                    .take()
                    .map(|active| (active.generation, active.handle));
            }
        }
    }

    if let Some((generation, handle)) = finished {
        match handle.join() {
            Ok(outcome) => apply_outcome(model, generation, outcome),
            Err(_) => {
                if generation == current_generation() {
                    model.factal_stream_status = "error".into();
                    model.push_log("Factal poll worker panicked before returning data.".into());
                }
            }
        }
    }

    if key.is_empty() {
        if model.factal_stream_status != "demo" {
            model.factal_stream_status = "demo".into();
        }
        return;
    }

    let mut should_spawn = false;
    let mut spawn_generation = 0_u64;
    {
        let mut manager = manager().lock().unwrap();
        if !manager.shutdown
            && manager.active.is_none()
            && now >= manager.next_poll_at.unwrap_or(now)
        {
            should_spawn = true;
            spawn_generation = manager.generation;
            manager.next_poll_at = Some(now + POLL_INTERVAL);
        }
    }

    if should_spawn {
        let api_key = key;
        let handle = thread::spawn(move || fetch_latest_events(&api_key));

        let mut manager = manager().lock().unwrap();
        if !manager.shutdown && manager.generation == spawn_generation {
            manager.active = Some(ActivePoll {
                generation: spawn_generation,
                handle,
            });
            if model.factal_stream_status != "live" {
                model.factal_stream_status = "syncing".into();
            }
        }
    }
}

pub fn invalidate() {
    let now = Instant::now();
    let mut manager = manager().lock().unwrap();
    manager.generation = manager.generation.wrapping_add(1);
    manager.next_poll_at = Some(now);
    manager.shutdown = false;
}

pub fn shutdown() {
    let mut manager = manager().lock().unwrap();
    manager.generation = manager.generation.wrapping_add(1);
    manager.shutdown = true;
    manager.next_poll_at = None;
}

fn apply_outcome(model: &mut AppModel, generation: u64, outcome: PollOutcome) {
    if generation != current_generation() {
        return;
    }

    match outcome {
        PollOutcome::Success(events) => {
            let count = events.len();
            let should_log = model.factal_stream_status != "live" || model.events.len() != count;
            model.replace_factal_events(events);
            model.factal_stream_status = "live".into();
            if should_log {
                model.push_log(format!(
                    "Factal sync loaded {} geolocated live event(s).",
                    count
                ));
            }
        }
        PollOutcome::AuthError => {
            let was_status = model.factal_stream_status.clone();
            model.factal_stream_status = "auth error".into();
            if was_status != "auth error" {
                model.push_log(
                    "Factal API rejected the configured token; check the API key.".into(),
                );
            }
        }
        PollOutcome::RateLimited => {
            let was_status = model.factal_stream_status.clone();
            model.factal_stream_status = "rate limited".into();
            if was_status != "rate limited" {
                model.push_log(
                    "Factal API rate limited the live poll; the app will retry automatically."
                        .into(),
                );
            }
        }
        PollOutcome::Error(error) => {
            model.factal_stream_status = "error".into();
            model.push_log(format!("Factal poll failed: {}", error));
        }
    }
}

fn current_generation() -> u64 {
    manager().lock().unwrap().generation
}

fn manager() -> &'static Mutex<PollManager> {
    static MANAGER: OnceLock<Mutex<PollManager>> = OnceLock::new();
    MANAGER.get_or_init(|| {
        Mutex::new(PollManager {
            active: None,
            generation: 0,
            tracked_key: String::new(),
            next_poll_at: Some(Instant::now()),
            shutdown: false,
        })
    })
}

fn fetch_latest_events(api_key: &str) -> PollOutcome {
    let client = match Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => return PollOutcome::Error(error.to_string()),
    };

    let mut headers = HeaderMap::new();
    let token = format!("Token {}", api_key.trim());
    let token_value = match HeaderValue::from_str(&token) {
        Ok(value) => value,
        Err(error) => return PollOutcome::Error(error.to_string()),
    };
    headers.insert(AUTHORIZATION, token_value);

    let response = match client.get(FACTAL_LATEST_URL).headers(headers).send() {
        Ok(response) => response,
        Err(error) => return PollOutcome::Error(error.to_string()),
    };

    match response.status().as_u16() {
        200 => {}
        401 | 403 => return PollOutcome::AuthError,
        429 => return PollOutcome::RateLimited,
        code => {
            return PollOutcome::Error(format!("Factal API returned unexpected status {}", code));
        }
    }

    let body = match response.text() {
        Ok(body) => body,
        Err(error) => return PollOutcome::Error(error.to_string()),
    };
    let payload: Value = match serde_json::from_str(&body) {
        Ok(payload) => payload,
        Err(error) => return PollOutcome::Error(error.to_string()),
    };

    let mut ids = HashSet::new();
    let mut events = Vec::new();
    for raw_event in payload
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(event) = parse_event(raw_event) {
            if ids.insert(event.id.clone()) {
                events.push(event);
            }
        }
    }

    PollOutcome::Success(events)
}

fn parse_event(raw: &Value) -> Option<EventRecord> {
    let id = string_value(raw.get("id")?)?;
    let occurred_at_raw = raw.get("date").and_then(Value::as_str).map(str::to_owned);
    let occurred_at = occurred_at_raw
        .as_deref()
        .unwrap_or("Unknown timestamp")
        .replace('T', " ");
    let summary = normalize_text(raw.get("content").and_then(Value::as_str).unwrap_or(""));
    let severity = classify_severity(raw.get("severity"));
    let severity_value = raw.get("severity").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_f64().map(|value| value as i64))
    });

    let topics = raw.get("topics").and_then(Value::as_array)?;
    let mut title = None;
    let mut location = None;
    let mut location_name = None;
    let mut point_wkt = None;
    let mut vertical = None;
    let mut subvertical = None;
    let mut topic_names = Vec::new();

    for wrapper in topics {
        let Some(topic) = wrapper.get("topic") else {
            continue;
        };

        if let Some(topic_name) = topic
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            topic_names.push(topic_name.to_owned());
        }

        if title.is_none() {
            title = topic
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
        }

        if location.is_none() {
            let lat = value_as_f32(topic.get("latitude"));
            let lon = value_as_f32(topic.get("longitude"));
            if let (Some(lat), Some(lon)) = (lat, lon) {
                location = Some(GeoPoint { lat, lon });
                point_wkt = topic
                    .get("point")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                location_name = topic
                    .get("point")
                    .and_then(Value::as_str)
                    .or_else(|| topic.get("name").and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
            }
        }

        if let Some(category) = topic
            .get("category")
            .and_then(Value::as_str)
            .or_else(|| topic.get("kind").and_then(Value::as_str))
        {
            match category.to_ascii_lowercase().as_str() {
                "vertical" => {
                    if vertical.is_none() {
                        vertical = topic
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned);
                    }
                }
                "subvertical" => {
                    if subvertical.is_none() {
                        subvertical = topic
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned);
                    }
                }
                _ => {}
            }
        }
    }

    let location = location?;
    let title = title.unwrap_or_else(|| {
        summary
            .split('.')
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Factal live event")
            .to_owned()
    });

    Some(EventRecord {
        id: format!("factal-{}", id),
        title,
        summary: if summary.is_empty() {
            "Live event synced from the Factal API.".into()
        } else {
            summary
        },
        severity,
        location_name: location_name
            .unwrap_or_else(|| format!("{:.4}, {:.4}", location.lat, location.lon)),
        location,
        source: "Factal API".into(),
        occurred_at,
        factal_brief: Some(FactalBrief {
            factal_id: id,
            severity_value,
            occurred_at_raw,
            point_wkt,
            vertical,
            subvertical,
            topics: topic_names,
            content: raw
                .get("content")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            raw_json_pretty: serde_json::to_string_pretty(raw).unwrap_or_else(|_| raw.to_string()),
        }),
    })
}

fn classify_severity(value: Option<&Value>) -> EventSeverity {
    let severity = value
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_f64().map(|value| value as i64))
        })
        .unwrap_or(0);

    if severity >= 4 {
        EventSeverity::Critical
    } else if severity >= 2 {
        EventSeverity::Elevated
    } else {
        EventSeverity::Advisory
    }
}

fn normalize_text(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn string_value(value: &Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        Some(value.to_owned())
    } else if let Some(value) = value.as_i64() {
        Some(value.to_string())
    } else if let Some(value) = value.as_u64() {
        Some(value.to_string())
    } else {
        value.as_f64().map(|value| value.to_string())
    }
}

fn value_as_f32(value: Option<&Value>) -> Option<f32> {
    let value = value?;
    if let Some(number) = value.as_f64() {
        Some(number as f32)
    } else if let Some(number) = value.as_i64() {
        Some(number as f32)
    } else if let Some(text) = value.as_str() {
        text.parse::<f32>().ok()
    } else {
        None
    }
}
