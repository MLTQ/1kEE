//! Cache-first public ALPR source integration for the DeFlock map layer.
//!
//! The renderer receives immutable public location snapshots; this module owns
//! cache I/O, refresh scheduling, and OpenStreetMap/Overpass parsing off the
//! UI thread. It intentionally does not handle feeds, routing, or avoidance.

use crate::model::{AppModel, GeoPoint};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CACHE_FILE: &str = "deflock_alpr_locations.json";
const CACHE_SCHEMA: u64 = 1;
const REFRESH_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
const INITIAL_BACKOFF: Duration = Duration::from_secs(5 * 60);
const MAX_BACKOFF: Duration = Duration::from_secs(2 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const DEFLOCK_DATA_ENDPOINT: &str = "https://data.dontgetflocked.com/cameras-us.json";
const OVERPASS_ENDPOINTS: &[&str] = &[
    "https://overpass-api.de/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
];
const OVERPASS_QUERY: &str = r#"[out:json][timeout:75];
area["ISO3166-1"="US"]->.us;
(
  node["man_made"="surveillance"]["surveillance:type"="ALPR"](area.us);
  way["man_made"="surveillance"]["surveillance:type"="ALPR"](area.us);
);
out meta center;"#;

/// Public DeFlock-compatible ALPR metadata sourced from OpenStreetMap.
///
/// This deliberately excludes submission, account, device, and contributor
/// identity data. It is sufficient for a transparent public map overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct DeflockAlprLocation {
    pub osm_id: i64,
    pub osm_type: String,
    pub location: GeoPoint,
    pub operator: Option<String>,
    pub brand: Option<String>,
    pub direction_degrees: Option<f32>,
    pub direction_cardinal: Option<String>,
    pub surveillance_zone: Option<String>,
    pub mount_type: Option<String>,
    pub reference: Option<String>,
    pub start_date: Option<String>,
    pub osm_timestamp: Option<String>,
    pub osm_version: Option<u32>,
}

/// DeFlock's public map endpoint for human-facing provenance.
pub const DEFLOCK_PROJECT_URL: &str = "https://maps.deflock.org/";
/// OpenStreetMap's required attribution target for the public source data.
pub const OPENSTREETMAP_ATTRIBUTION_URL: &str = "https://www.openstreetmap.org/copyright";
/// Short provenance text suitable for a map legend or status panel.
pub const ATTRIBUTION: &str =
    "ALPR locations: DeFlock-compatible OpenStreetMap data © OpenStreetMap contributors";

#[derive(Clone)]
struct CachedSnapshot {
    fetched_at_unix: u64,
    locations: Vec<DeflockAlprLocation>,
}

enum WorkerOutcome {
    Cached {
        generation: u64,
        locations: Vec<DeflockAlprLocation>,
        fetched_at_unix: u64,
        stale: bool,
    },
    Refreshed {
        generation: u64,
        locations: Vec<DeflockAlprLocation>,
        cache_warning: Option<String>,
    },
    Failed {
        generation: u64,
        message: String,
        cache_was_available: bool,
    },
}

struct ActiveWorker {
    receiver: Receiver<WorkerOutcome>,
}

struct SourceState {
    cache_path: Option<PathBuf>,
    generation: u64,
    worker: Option<ActiveWorker>,
    next_attempt: Option<Instant>,
    failures: u8,
}

impl Default for SourceState {
    fn default() -> Self {
        Self {
            cache_path: None,
            generation: 0,
            worker: None,
            next_attempt: None,
            failures: 0,
        }
    }
}

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

fn source_state() -> &'static Mutex<SourceState> {
    static STATE: OnceLock<Mutex<SourceState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(SourceState::default()))
}

/// Advances the non-blocking source lifecycle. Local cache data is applied
/// before any refresh completes; network work always runs on a worker thread.
pub fn tick(model: &mut AppModel) {
    if SHUTTING_DOWN.load(Ordering::Relaxed) {
        return;
    }

    let cache_path = cache_path(model.selected_root.as_deref());
    let now = Instant::now();
    let mut state = source_state()
        .lock()
        .expect("Deflock source state poisoned");

    if state.cache_path.as_ref() != Some(&cache_path) {
        state.cache_path = Some(cache_path.clone());
        state.generation = state.generation.wrapping_add(1);
        state.worker = None;
        state.next_attempt = None;
        state.failures = 0;
        model.replace_deflock_alpr_locations(Vec::new());
        model.deflock_status = "loading local ALPR cache…".into();
    }

    let mut outcomes = Vec::new();
    let mut disconnected = false;
    if let Some(worker) = state.worker.as_ref() {
        loop {
            match worker.receiver.try_recv() {
                Ok(outcome) => outcomes.push(outcome),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
    }
    if disconnected {
        state.worker = None;
    }

    for outcome in outcomes {
        apply_outcome(model, &mut state, outcome, now);
    }

    let due = state.next_attempt.map(|at| now >= at).unwrap_or(true);
    if state.worker.is_none() && due {
        let generation = state.generation;
        state.worker = Some(ActiveWorker {
            receiver: spawn_worker(generation, cache_path),
        });
        if model.deflock_alpr_locations.is_empty() {
            model.deflock_status = "loading local ALPR cache…".into();
        }
    }
}

fn apply_outcome(
    model: &mut AppModel,
    state: &mut SourceState,
    outcome: WorkerOutcome,
    now: Instant,
) {
    let generation = match &outcome {
        WorkerOutcome::Cached { generation, .. }
        | WorkerOutcome::Refreshed { generation, .. }
        | WorkerOutcome::Failed { generation, .. } => *generation,
    };
    if generation != state.generation {
        return;
    }

    match outcome {
        WorkerOutcome::Cached {
            locations,
            fetched_at_unix,
            stale,
            ..
        } => {
            let count = locations.len();
            model.replace_deflock_alpr_locations(locations);
            let age = format_age(now_unix().saturating_sub(fetched_at_unix));
            model.deflock_status = if stale {
                format!("cached {count} ALPR locations ({age} old); refreshing…")
            } else {
                format!("cached {count} ALPR locations ({age} old)")
            };
            state.failures = 0;
            state.next_attempt = Some(now + REFRESH_INTERVAL);
        }
        WorkerOutcome::Refreshed {
            locations,
            cache_warning,
            ..
        } => {
            let count = locations.len();
            model.replace_deflock_alpr_locations(locations);
            model.deflock_status = cache_warning.map_or_else(
                || format!("refreshed {count} public ALPR locations"),
                |warning| {
                    format!("loaded {count} public ALPR locations; cache write failed ({warning})")
                },
            );
            state.failures = 0;
            state.next_attempt = Some(now + REFRESH_INTERVAL);
        }
        WorkerOutcome::Failed {
            message,
            cache_was_available,
            ..
        } => {
            state.failures = state.failures.saturating_add(1);
            let delay = backoff_for(state.failures);
            state.next_attempt = Some(now + delay);
            let retained = cache_was_available || !model.deflock_alpr_locations.is_empty();
            model.deflock_status = if retained {
                format!("cached ALPR locations retained; refresh failed ({message})")
            } else {
                format!("ALPR cache unavailable; refresh failed ({message})")
            };
        }
    }
}

fn spawn_worker(generation: u64, path: PathBuf) -> Receiver<WorkerOutcome> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let cached = load_cache(&path);
        let cache_was_available = cached.is_ok();
        let mut needs_refresh = true;
        if let Ok(snapshot) = cached {
            let stale =
                now_unix().saturating_sub(snapshot.fetched_at_unix) >= REFRESH_INTERVAL.as_secs();
            if send_outcome(
                &sender,
                WorkerOutcome::Cached {
                    generation,
                    locations: snapshot.locations,
                    fetched_at_unix: snapshot.fetched_at_unix,
                    stale,
                },
            ) {
                return;
            }
            needs_refresh = stale;
        }

        if !needs_refresh || SHUTTING_DOWN.load(Ordering::Relaxed) {
            return;
        }

        match fetch_locations() {
            Ok(locations) => {
                let cache_warning = save_cache(&path, &locations).err();
                let _ = send_outcome(
                    &sender,
                    WorkerOutcome::Refreshed {
                        generation,
                        locations,
                        cache_warning,
                    },
                );
            }
            Err(error) => {
                let _ = send_outcome(
                    &sender,
                    WorkerOutcome::Failed {
                        generation,
                        message: error,
                        cache_was_available,
                    },
                );
            }
        }
    });
    receiver
}

/// Returns true when the worker should stop sending further updates.
fn send_outcome(sender: &mpsc::Sender<WorkerOutcome>, outcome: WorkerOutcome) -> bool {
    if SHUTTING_DOWN.load(Ordering::Relaxed) {
        return true;
    }
    let sent = sender.send(outcome).is_ok();
    if sent {
        crate::app::request_repaint();
    }
    !sent
}

fn fetch_locations() -> Result<Vec<DeflockAlprLocation>, String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(REQUEST_TIMEOUT)
        .user_agent("1kEE public ALPR cache importer")
        .build()
        .map_err(|error| error.to_string())?;

    let mut errors = Vec::new();
    match client
        .get(DEFLOCK_DATA_ENDPOINT)
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.text())
    {
        Ok(body) => match parse_deflock_geojson(&body) {
            Ok(locations) => return Ok(locations),
            Err(error) => errors.push(format!("DeFlock data: {error}")),
        },
        Err(error) => errors.push(format!("DeFlock data: {error}")),
    }

    for endpoint in OVERPASS_ENDPOINTS {
        match client
            .post(*endpoint)
            .form(&[("data", OVERPASS_QUERY)])
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.text())
        {
            Ok(body) => match parse_overpass_json(&body) {
                Ok(locations) => return Ok(locations),
                Err(error) => errors.push(format!("{endpoint}: {error}")),
            },
            Err(error) => errors.push(format!("{endpoint}: {error}")),
        }
    }

    Err(format!(
        "all public ALPR sources failed: {}",
        errors.join("; ")
    ))
}

fn cache_path(selected_root: Option<&Path>) -> PathBuf {
    if let Some(data_root) = crate::settings_store::configured_data_root() {
        return data_root.join(CACHE_FILE);
    }
    selected_root
        .map(Path::to_path_buf)
        .or_else(crate::settings_store::effective_asset_root)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Data")
        .join(CACHE_FILE)
}

fn load_cache(path: &Path) -> Result<CachedSnapshot, String> {
    let body = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let value: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let schema = value
        .get("schema")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if schema != CACHE_SCHEMA {
        return Err(format!("unsupported cache schema {schema}"));
    }
    let fetched_at_unix = value
        .get("fetched_at_unix")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let entries = value
        .get("locations")
        .and_then(Value::as_array)
        .ok_or_else(|| "cache has no locations array".to_owned())?;
    let mut locations: Vec<_> = entries.iter().filter_map(parse_cached_location).collect();
    normalise_locations(&mut locations);
    ensure_nonempty(locations, "cache").map(|locations| CachedSnapshot {
        fetched_at_unix,
        locations,
    })
}

fn save_cache(path: &Path, locations: &[DeflockAlprLocation]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "ALPR cache path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let value = json!({
        "schema": CACHE_SCHEMA,
        "fetched_at_unix": now_unix(),
        "source": "DeFlock / OpenStreetMap public ALPR snapshot",
        "attribution": ATTRIBUTION,
        "locations": locations.iter().map(location_to_value).collect::<Vec<_>>(),
    });
    let body = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, body).map_err(|error| error.to_string())?;
    fs::File::open(&temporary)
        .and_then(|file| file.sync_data())
        .map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

fn parse_overpass_json(body: &str) -> Result<Vec<DeflockAlprLocation>, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let entries = value
        .get("elements")
        .and_then(Value::as_array)
        .ok_or_else(|| "Overpass response has no elements array".to_owned())?;
    let mut locations: Vec<_> = entries.iter().filter_map(parse_overpass_element).collect();
    normalise_locations(&mut locations);
    ensure_nonempty(locations, "Overpass response")
}

/// Parse the canonical DeFlock public GeoJSON snapshot. The public worker uses
/// OSM-derived point features, so only the same map metadata we support from
/// Overpass is retained.
fn parse_deflock_geojson(body: &str) -> Result<Vec<DeflockAlprLocation>, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let features = value
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| "DeFlock data has no GeoJSON features array".to_owned())?;
    let mut locations: Vec<_> = features.iter().filter_map(parse_deflock_feature).collect();
    normalise_locations(&mut locations);
    ensure_nonempty(locations, "DeFlock data")
}

fn parse_deflock_feature(value: &Value) -> Option<DeflockAlprLocation> {
    let properties = value.get("properties")?.as_object()?;
    let geometry = value.get("geometry")?.as_object()?;
    if geometry.get("type")?.as_str()? != "Point" {
        return None;
    }
    let coordinates = geometry.get("coordinates")?.as_array()?;
    let lon = coordinates.first()?.as_f64()? as f32;
    let lat = coordinates.get(1)?.as_f64()? as f32;
    if !valid_point(lat, lon) {
        return None;
    }

    let osm_id = integer_tag(Some(properties), "osmId")
        .or_else(|| integer_tag(Some(properties), "osm_id"))
        .or_else(|| value.get("id").and_then(value_i64))?;
    let osm_type = tag(Some(properties), "osmType")
        .or_else(|| tag(Some(properties), "osm_type"))
        .filter(|kind| matches!(kind.as_str(), "node" | "way"))?;
    let direction_text = tag(Some(properties), "direction")
        .or_else(|| tag(Some(properties), "directionDegrees"))
        .or_else(|| tag(Some(properties), "direction_degrees"));
    let (direction_degrees, direction_cardinal) = parse_direction(direction_text.as_deref());

    Some(DeflockAlprLocation {
        osm_id,
        osm_type,
        location: GeoPoint { lat, lon },
        operator: tag(Some(properties), "operator"),
        brand: tag(Some(properties), "brand").or_else(|| tag(Some(properties), "manufacturer")),
        direction_degrees,
        direction_cardinal,
        surveillance_zone: tag(Some(properties), "surveillanceZone")
            .or_else(|| tag(Some(properties), "surveillance_zone")),
        mount_type: tag(Some(properties), "mountType")
            .or_else(|| tag(Some(properties), "mount_type")),
        reference: tag(Some(properties), "ref"),
        start_date: tag(Some(properties), "startDate")
            .or_else(|| tag(Some(properties), "start_date")),
        osm_timestamp: tag(Some(properties), "osmTimestamp")
            .or_else(|| tag(Some(properties), "osm_timestamp")),
        osm_version: integer_tag(Some(properties), "osmVersion")
            .or_else(|| integer_tag(Some(properties), "osm_version"))
            .and_then(|version| u32::try_from(version).ok()),
    })
}

fn ensure_nonempty(
    locations: Vec<DeflockAlprLocation>,
    source: &str,
) -> Result<Vec<DeflockAlprLocation>, String> {
    if locations.is_empty() {
        Err(format!("{source} contained no valid public ALPR locations"))
    } else {
        Ok(locations)
    }
}

fn parse_overpass_element(value: &Value) -> Option<DeflockAlprLocation> {
    let osm_id = value.get("id")?.as_i64()?;
    let osm_type = value.get("type")?.as_str()?.to_owned();
    if !matches!(osm_type.as_str(), "node" | "way") {
        return None;
    }
    let location = parse_coordinates(value)?;
    let tags = value.get("tags").and_then(Value::as_object)?;
    if !tag_equals(Some(tags), "man_made", "surveillance")
        || !tag_equals(Some(tags), "surveillance:type", "ALPR")
    {
        return None;
    }
    let tags = Some(tags);
    let direction_text = tag(tags, "camera:direction").or_else(|| tag(tags, "direction"));
    let (direction_degrees, direction_cardinal) = parse_direction(direction_text.as_deref());
    Some(DeflockAlprLocation {
        osm_id,
        osm_type,
        location,
        operator: tag(tags, "operator"),
        brand: tag(tags, "brand").or_else(|| tag(tags, "manufacturer")),
        direction_degrees,
        direction_cardinal,
        surveillance_zone: tag(tags, "surveillance:zone"),
        mount_type: tag(tags, "camera:mount").or_else(|| tag(tags, "mount")),
        reference: tag(tags, "ref"),
        start_date: tag(tags, "start_date"),
        osm_timestamp: value
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_owned),
        osm_version: value
            .get("version")
            .and_then(Value::as_u64)
            .and_then(|version| u32::try_from(version).ok()),
    })
}

fn parse_cached_location(value: &Value) -> Option<DeflockAlprLocation> {
    let osm_id = value.get("osm_id")?.as_i64()?;
    let osm_type = value.get("osm_type")?.as_str()?.to_owned();
    if !matches!(osm_type.as_str(), "node" | "way") {
        return None;
    }
    let lat = value.get("lat")?.as_f64()? as f32;
    let lon = value.get("lon")?.as_f64()? as f32;
    valid_point(lat, lon).then_some(DeflockAlprLocation {
        osm_id,
        osm_type,
        location: GeoPoint { lat, lon },
        operator: string_field(value, "operator"),
        brand: string_field(value, "brand"),
        direction_degrees: value
            .get("direction_degrees")
            .and_then(Value::as_f64)
            .map(|direction| direction as f32)
            .filter(|direction| direction.is_finite()),
        direction_cardinal: string_field(value, "direction_cardinal"),
        surveillance_zone: string_field(value, "surveillance_zone"),
        mount_type: string_field(value, "mount_type"),
        reference: string_field(value, "reference"),
        start_date: string_field(value, "start_date"),
        osm_timestamp: string_field(value, "osm_timestamp"),
        osm_version: value
            .get("osm_version")
            .and_then(Value::as_u64)
            .and_then(|version| u32::try_from(version).ok()),
    })
}

fn location_to_value(location: &DeflockAlprLocation) -> Value {
    json!({
        "osm_id": location.osm_id,
        "osm_type": location.osm_type,
        "lat": location.location.lat,
        "lon": location.location.lon,
        "operator": location.operator,
        "brand": location.brand,
        "direction_degrees": location.direction_degrees,
        "direction_cardinal": location.direction_cardinal,
        "surveillance_zone": location.surveillance_zone,
        "mount_type": location.mount_type,
        "reference": location.reference,
        "start_date": location.start_date,
        "osm_timestamp": location.osm_timestamp,
        "osm_version": location.osm_version,
    })
}

fn parse_coordinates(value: &Value) -> Option<GeoPoint> {
    let (lat, lon) = value
        .get("lat")
        .and_then(Value::as_f64)
        .zip(value.get("lon").and_then(Value::as_f64))
        .or_else(|| {
            value
                .get("center")
                .and_then(Value::as_object)
                .and_then(|center| {
                    center
                        .get("lat")
                        .and_then(Value::as_f64)
                        .zip(center.get("lon").and_then(Value::as_f64))
                })
        })?;
    let (lat, lon) = (lat as f32, lon as f32);
    valid_point(lat, lon).then_some(GeoPoint { lat, lon })
}

fn valid_point(lat: f32, lon: f32) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
}

fn tag(tags: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<String> {
    tags?
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn tag_equals(tags: Option<&serde_json::Map<String, Value>>, key: &str, expected: &str) -> bool {
    tag(tags, key).is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn integer_tag(tags: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<i64> {
    tags?.get(key).and_then(value_i64)
}

fn value_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn parse_direction(value: Option<&str>) -> (Option<f32>, Option<String>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return (None, None);
    };
    let numeric = value
        .trim_end_matches('°')
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|direction| direction.is_finite())
        .map(|direction| direction.rem_euclid(360.0));
    if numeric.is_some() {
        return (numeric, None);
    }
    let cardinal = value.to_ascii_uppercase();
    let degrees = match cardinal.as_str() {
        "N" => Some(0.0),
        "NE" => Some(45.0),
        "E" => Some(90.0),
        "SE" => Some(135.0),
        "S" => Some(180.0),
        "SW" => Some(225.0),
        "W" => Some(270.0),
        "NW" => Some(315.0),
        _ => None,
    };
    (degrees, degrees.map(|_| cardinal))
}

fn normalise_locations(locations: &mut Vec<DeflockAlprLocation>) {
    locations.sort_by(|left, right| {
        left.osm_type
            .cmp(&right.osm_type)
            .then(left.osm_id.cmp(&right.osm_id))
    });
    locations
        .dedup_by(|left, right| left.osm_type == right.osm_type && left.osm_id == right.osm_id);
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_age(seconds: u64) -> String {
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn backoff_for(failures: u8) -> Duration {
    let multiplier = 1u64 << u32::from(failures.saturating_sub(1).min(8));
    INITIAL_BACKOFF
        .checked_mul(multiplier as u32)
        .unwrap_or(MAX_BACKOFF)
        .min(MAX_BACKOFF)
}

/// Retires source workers during application shutdown. The blocking request is
/// bounded by its HTTP timeout; generation checks prevent stale application.
pub fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::Relaxed);
    if let Ok(mut state) = source_state().lock() {
        state.generation = state.generation.wrapping_add(1);
        state.worker = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overpass_parser_accepts_nodes_and_way_centers_with_stable_deduplication() {
        let body = r#"{
          "elements": [
            {"type":"way","id":7,"center":{"lat":40.71,"lon":-74.0},"tags":{"man_made":"surveillance","surveillance:type":"ALPR","operator":"City","direction":"NW"}},
            {"type":"node","id":3,"lat":40.72,"lon":-73.99,"tags":{"man_made":"surveillance","surveillance:type":"ALPR","camera:direction":"90","brand":"Acme"}},
            {"type":"node","id":3,"lat":0.0,"lon":0.0,"tags":{}},
            {"type":"node","id":9,"lat":95.0,"lon":0.0,"tags":{}}
          ]
        }"#;

        let locations = parse_overpass_json(body).unwrap();
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].osm_type, "node");
        assert_eq!(locations[0].osm_id, 3);
        assert_eq!(locations[0].direction_degrees, Some(90.0));
        assert_eq!(locations[1].osm_type, "way");
        assert_eq!(locations[1].direction_degrees, Some(315.0));
        assert_eq!(locations[1].direction_cardinal.as_deref(), Some("NW"));
    }

    #[test]
    fn source_parsers_reject_empty_or_unqualified_data() {
        assert!(parse_overpass_json(r#"{"elements":[]}"#).is_err());
        assert!(parse_overpass_json(
            r#"{"elements":[{"type":"node","id":1,"lat":1.0,"lon":2.0,"tags":{"man_made":"tower"}}]}"#,
        )
        .is_err());
        let geojson = r#"{
          "type":"FeatureCollection",
          "features":[{
            "type":"Feature",
            "geometry":{"type":"Point","coordinates":[-73.99,40.72]},
            "properties":{"osmId":9,"osmType":"node","direction":"E","operator":"City"}
          }]
        }"#;
        let locations = parse_deflock_geojson(geojson).unwrap();
        assert_eq!(locations[0].osm_id, 9);
        assert_eq!(locations[0].direction_degrees, Some(90.0));
    }

    #[test]
    fn cache_round_trip_retains_public_metadata() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "one_thousand_electric_eye_deflock_{}_{}",
            std::process::id(),
            unique
        ));
        let path = dir.join(CACHE_FILE);
        let locations = vec![DeflockAlprLocation {
            osm_id: 42,
            osm_type: "node".into(),
            location: GeoPoint {
                lat: 1.25,
                lon: -2.5,
            },
            operator: Some("public operator".into()),
            brand: None,
            direction_degrees: Some(270.0),
            direction_cardinal: None,
            surveillance_zone: Some("traffic".into()),
            mount_type: None,
            reference: Some("ALPR-42".into()),
            start_date: None,
            osm_timestamp: None,
            osm_version: Some(3),
        }];

        save_cache(&path, &locations).unwrap();
        let loaded = load_cache(&path).unwrap();
        assert_eq!(loaded.locations, locations);
        fs::remove_dir_all(dir).unwrap();
    }
}
