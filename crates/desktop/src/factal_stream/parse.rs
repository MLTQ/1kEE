use crate::model::{EventRecord, EventSeverity, FactalBrief, GeoPoint};
use serde_json::Value;

pub(super) fn parse_event(raw: &Value) -> Option<EventRecord> {
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
