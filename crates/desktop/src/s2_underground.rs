//! S2Underground ArcGIS Feature Service integration.
//!
//! All 10 publicly-accessible FeatureServer layers are defined in `LAYERS`.
//! Polling is non-blocking: `poll_enabled` returns cached data immediately and
//! spawns background threads for stale layers.  A 5-minute poll interval is
//! used since the data changes slowly.
//!
//! The shared HTTP client and per-layer caches follow the same OnceLock pattern
//! as `flight_tracks`.

use crate::model::{GeoPoint, S2Event};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ── HTTP client ───────────────────────────────────────────────────────────────

fn http_client() -> &'static reqwest::blocking::Client {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("failed to build S2 HTTP client")
    })
}

// ── Layer definitions ─────────────────────────────────────────────────────────

const BASE: &str =
    "https://services.arcgis.com/OeCRCKr7XFYQNdyJ/arcgis/rest/services";

pub const POLL_INTERVAL: Duration = Duration::from_secs(300); // 5 min

pub struct S2LayerDef {
    /// Short identifier used as a HashMap key and stored on S2Event.
    pub key: &'static str,
    /// Human-readable name shown in the Items tab.
    pub display_name: &'static str,
    /// ArcGIS service name (part of the URL).
    pub service: &'static str,
    /// Layer ID within the FeatureServer (0 if unknown/single-layer).
    pub layer_id: u32,
    /// Globe marker colour for this layer (constant; does not vary per theme
    /// since event markers need high-contrast visibility on all themes).
    pub color: (u8, u8, u8),
}

pub const LAYERS: &[S2LayerDef] = &[
    S2LayerDef {
        key: "europe_kinetic",
        display_name: "Europe — Kinetic Events",
        service: "Kinetic_Activity_Tracker_Europe",
        layer_id: 22,
        color: (255, 120, 50),  // orange-red
    },
    S2LayerDef {
        key: "oconus_kinetic",
        display_name: "OCONUS — Kinetic Events",
        service: "KineticActivitiesTrackerOCONUS",
        layer_id: 21,
        color: (255, 140, 60),  // orange
    },
    S2LayerDef {
        key: "domestic_terror",
        display_name: "Domestic Terrorism (US)",
        service: "Domestic_Terrorism_Tracker",
        layer_id: 0,
        color: (210, 50, 50),   // crimson
    },
    S2LayerDef {
        key: "drone_reports",
        display_name: "Drone / UAV Reports",
        service: "Drone_Reports",
        layer_id: 0,
        color: (60, 200, 195),  // teal
    },
    S2LayerDef {
        key: "iran_cip",
        display_name: "Iran — Common Intel Picture",
        service: "Iran_Common_Intelligence_Picture",
        layer_id: 0,
        color: (230, 175, 40),  // amber
    },
    S2LayerDef {
        key: "iran_kinetic",
        display_name: "Iran — Kinetic Events",
        service: "Iran_Kinetic_Events",
        layer_id: 0,
        color: (240, 150, 30),  // amber-orange
    },
    S2LayerDef {
        key: "venezuela",
        display_name: "Venezuela — Kinetic Events",
        service: "Venezuelan_Crisis_Kinetic_Events",
        layer_id: 0,
        color: (200, 220, 60),  // yellow-green
    },
    S2LayerDef {
        key: "border_crisis",
        display_name: "US Border Crisis",
        service: "Border_Crisis_Incident_Tracker",
        layer_id: 0,
        color: (255, 165, 40),  // warm orange
    },
    S2LayerDef {
        key: "cultural_crime",
        display_name: "Cultural Crime",
        service: "Cultural_Crime_Tracker",
        layer_id: 0,
        color: (170, 90, 220),  // purple
    },
    S2LayerDef {
        key: "uk_migration",
        display_name: "UK Migration Crisis",
        service: "UK_Migration_Crisis",
        layer_id: 0,
        color: (100, 175, 235), // steel blue
    },
];

/// Return the egui Color32 for a given layer key (falls back to grey).
pub fn layer_color(key: &str) -> egui::Color32 {
    LAYERS
        .iter()
        .find(|l| l.key == key)
        .map(|l| egui::Color32::from_rgb(l.color.0, l.color.1, l.color.2))
        .unwrap_or(egui::Color32::GRAY)
}

/// Return the S2LayerDef for a given key, or None.
pub fn layer_def(key: &str) -> Option<&'static S2LayerDef> {
    LAYERS.iter().find(|l| l.key == key)
}

// ── Per-layer cache ───────────────────────────────────────────────────────────

struct LayerCache {
    events: Vec<S2Event>,
    last_poll: Option<Instant>,
    loading: bool,
    /// Human-readable status ("idle", "N events", "error: …").
    pub status: String,
}

fn all_caches() -> &'static Mutex<HashMap<&'static str, LayerCache>> {
    static C: OnceLock<Mutex<HashMap<&'static str, LayerCache>>> = OnceLock::new();
    C.get_or_init(|| {
        let mut m = HashMap::new();
        for layer in LAYERS {
            m.insert(
                layer.key,
                LayerCache {
                    events: Vec::new(),
                    last_poll: None,
                    loading: false,
                    status: "idle".into(),
                },
            );
        }
        Mutex::new(m)
    })
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Poll all enabled layers and return a merged Vec of all cached events.
/// Spawns background threads for stale layers; never blocks.
pub fn poll_enabled(
    enabled: &HashMap<String, bool>,
    ctx: egui::Context,
) -> Vec<S2Event> {
    for layer in LAYERS {
        let is_enabled = enabled.get(layer.key).copied().unwrap_or(false);
        if !is_enabled {
            continue;
        }

        // Check if a refresh is needed.
        let should_fetch = all_caches()
            .lock()
            .map(|c| {
                let entry = c.get(layer.key).expect("cache pre-populated");
                if entry.loading {
                    return false;
                }
                entry
                    .last_poll
                    .map(|t| t.elapsed() >= POLL_INTERVAL)
                    .unwrap_or(true)
            })
            .unwrap_or(false);

        if should_fetch {
            if let Ok(mut c) = all_caches().lock() {
                let entry = c.get_mut(layer.key).unwrap();
                entry.loading = true;
                entry.status = "loading\u{2026}".into();
            }
            let key = layer.key;
            let service = layer.service;
            let layer_id = layer.layer_id;
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let (events, status) = fetch_layer(key, service, layer_id);
                if let Ok(mut c) = all_caches().lock() {
                    let entry = c.get_mut(key).unwrap();
                    entry.events = events;
                    entry.loading = false;
                    entry.last_poll = Some(Instant::now());
                    entry.status = status;
                }
                ctx2.request_repaint();
            });
        }
    }

    // Collect all cached events for enabled layers.
    let Ok(c) = all_caches().lock() else {
        return Vec::new();
    };
    LAYERS
        .iter()
        .filter(|l| enabled.get(l.key).copied().unwrap_or(false))
        .flat_map(|l| c.get(l.key).map(|e| e.events.clone()).unwrap_or_default())
        .collect()
}

/// Status string for a given layer key ("idle", "N events", "error: …").
pub fn layer_status(key: &str) -> String {
    all_caches()
        .lock()
        .map(|c| {
            c.get(key)
                .map(|e| e.status.clone())
                .unwrap_or_else(|| "unknown".into())
        })
        .unwrap_or_else(|_| "error".into())
}

/// Invalidate a specific layer so it refetches on next poll.
pub fn invalidate(key: &str) {
    if let Ok(mut c) = all_caches().lock() {
        if let Some(entry) = c.get_mut(key) {
            entry.last_poll = None;
            entry.loading = false;
        }
    }
}

// ── Fetch ─────────────────────────────────────────────────────────────────────

fn fetch_layer(key: &'static str, service: &str, layer_id: u32) -> (Vec<S2Event>, String) {
    let url = format!(
        "{BASE}/{service}/FeatureServer/{layer_id}/query\
         ?where=1%3D1&outFields=*&returnGeometry=true&outSR=4326&f=json"
    );

    let resp = match http_client().get(&url).send() {
        Ok(r) => r,
        Err(e) => return (Vec::new(), format!("connect error: {e}")),
    };

    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        eprintln!("[s2] {key}: HTTP {code}");
        return (Vec::new(), format!("HTTP {code}"));
    }

    let text = match resp.text() {
        Ok(t) => t,
        Err(e) => return (Vec::new(), format!("read error: {e}")),
    };

    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), format!("parse error: {e}")),
    };

    // ArcGIS returns { "error": {...} } if the layer ID is wrong.
    if json.get("error").is_some() {
        let msg = json["error"]["message"]
            .as_str()
            .unwrap_or("unknown error")
            .to_owned();
        eprintln!("[s2] {key}: API error — {msg}");
        return (Vec::new(), format!("API error: {msg}"));
    }

    let features = match json["features"].as_array() {
        Some(f) => f,
        None => return (Vec::new(), "0 events".into()),
    };

    let events: Vec<S2Event> = features
        .iter()
        .filter_map(|f| parse_feature(f, key))
        .collect();

    let status = format!("{} events", events.len());
    (events, status)
}

fn parse_feature(feat: &serde_json::Value, layer_key: &'static str) -> Option<S2Event> {
    // Geometry: { "x": lon, "y": lat } in outSR=4326
    let geom = feat.get("geometry")?;
    let lon = geom["x"].as_f64()? as f32;
    let lat = geom["y"].as_f64()? as f32;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }

    let attr = feat.get("attributes")?;
    let s = |key: &str| -> Option<String> {
        attr[key]
            .as_str()
            .filter(|v| !v.is_empty() && *v != " ")
            .map(|v| v.trim().to_owned())
    };
    let i = |key: &str| -> Option<i32> {
        attr[key].as_i64().map(|v| v as i32).or_else(|| {
            attr[key].as_f64().map(|v| v as i32)
        })
    };

    let object_id = attr["OBJECTID"].as_i64().unwrap_or(0);
    // ArcGIS date fields are milliseconds since Unix epoch.
    let date_ms = attr["Date"].as_i64().or_else(|| attr["date"].as_i64());

    Some(S2Event {
        object_id,
        layer_key: layer_key.to_owned(),
        location: GeoPoint { lat, lon },
        date_ms,
        attack_type: s("AttackType"),
        motive: s("AttackMotive"),
        notes: s("Notes"),
        address: s("FullStreetAddress").or_else(|| s("GeolocationNotes")),
        civilian_killed: i("CivilianKilled"),
        civilian_wounded: i("CivilianWounded"),
        friendly_killed: i("FriendlyKilled"),
        friendly_wounded: i("FriendlyWounded"),
        enemy_killed: i("EnemyKilled"),
        enemy_wounded: i("EnemyWounded"),
    })
}

// ── Date formatting ───────────────────────────────────────────────────────────

/// Format a Unix-millisecond timestamp as "YYYY-MM-DD".
pub fn format_date(ms: i64) -> String {
    let total_days = (ms / 86_400_000).max(0);
    let (y, m, d) = days_to_ymd(total_days as u32);
    format!("{y}-{m:02}-{d:02}")
}

fn days_to_ymd(mut days: u32) -> (u32, u32, u32) {
    let mut year = 1970u32;
    loop {
        let diy = if is_leap(year) { 366 } else { 365 };
        if days < diy {
            break;
        }
        days -= diy;
        year += 1;
    }
    let leap = is_leap(year);
    let month_lengths: [u32; 12] = [
        31,
        if leap { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut month = 1u32;
    for &ml in &month_lengths {
        if days < ml {
            break;
        }
        days -= ml;
        month += 1;
    }
    (year, month, days + 1)
}

fn is_leap(y: u32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}
