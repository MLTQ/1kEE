//! General-purpose ArcGIS FeatureServer scraper.
//!
//! Users can paste ANY ArcGIS FeatureServer URL; the app auto-discovers the
//! service's layers via `{url}?f=json` and the user toggles individual layers.
//! The old 10 S2Underground services are pre-seeded as initial sources.
//!
//! All HTTP happens in background threads; `poll` returns cached data immediately.

use crate::model::{ArcGisFeature, ArcGisLayerDef, GeoPoint};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_secs(300);

/// Fixed color palette — cycles per (source_index, layer_index).
const PALETTE: &[(u8, u8, u8)] = &[
    (255, 120, 50),  // orange-red
    (60, 200, 195),  // teal
    (210, 50, 50),   // crimson
    (230, 175, 40),  // amber
    (170, 90, 220),  // purple
    (100, 175, 235), // steel blue
    (200, 220, 60),  // yellow-green
    (255, 165, 40),  // warm orange
];

pub fn palette_color(idx: usize) -> egui::Color32 {
    let (r, g, b) = PALETTE[idx % PALETTE.len()];
    egui::Color32::from_rgb(r, g, b)
}

struct LayerEntry {
    features: Vec<ArcGisFeature>,
    last_poll: Option<Instant>,
    loading: bool,
    pub status: String,
}

struct SourceCache {
    /// Discovered layer definitions. None = still discovering.
    pub layers: Option<Vec<ArcGisLayerDef>>,
    pub display_name: String,
    pub discovering: bool,
    pub discover_error: Option<String>,
    /// color_offset so each source gets a distinct hue band.
    pub color_offset: usize,
    layer_entries: HashMap<u32, LayerEntry>,
}

fn cache() -> &'static Mutex<HashMap<String, SourceCache>> {
    static C: OnceLock<Mutex<HashMap<String, SourceCache>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn http_client() -> &'static reqwest::blocking::Client {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("arcgis http client")
    })
}

/// Normalize a user-pasted URL to a canonical FeatureServer base URL.
/// Strips trailing slash, `/query`, `/0`, etc.
pub fn normalize_url(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('/');
    // Strip common suffixes
    let s = if let Some(i) = s.rfind("/FeatureServer") {
        &s[..i + "/FeatureServer".len()]
    } else {
        s
    };
    s.to_owned()
}

/// Register a new source and kick off layer discovery.
/// If the source is already registered, this is a no-op.
pub fn add_source(url: String, color_offset: usize, ctx: egui::Context) {
    let canonical = normalize_url(&url);
    {
        let Ok(mut c) = cache().lock() else { return };
        if c.contains_key(&canonical) {
            return; // already registered
        }
        c.insert(
            canonical.clone(),
            SourceCache {
                layers: None,
                display_name: short_name(&canonical),
                discovering: true,
                discover_error: None,
                color_offset,
                layer_entries: HashMap::new(),
            },
        );
    }
    let url2 = canonical.clone();
    let ctx2 = ctx.clone();
    std::thread::spawn(move || {
        let (layers, name, err) = discover_layers(&url2);
        if let Ok(mut c) = cache().lock() {
            if let Some(entry) = c.get_mut(&url2) {
                entry.discovering = false;
                if let Some(e) = err {
                    entry.discover_error = Some(e);
                } else {
                    entry.display_name = name;
                    entry.layers = Some(layers);
                }
            }
        }
        ctx2.request_repaint();
    });
}

/// Remove a source from the cache.
pub fn remove_source(url: &str) {
    let canonical = normalize_url(url);
    if let Ok(mut c) = cache().lock() {
        c.remove(&canonical);
    }
}

/// Returns (layers, service_name, error).
fn discover_layers(url: &str) -> (Vec<ArcGisLayerDef>, String, Option<String>) {
    let req_url = format!("{url}?f=json");
    let resp = match http_client().get(&req_url).send() {
        Ok(r) => r,
        Err(e) => return (vec![], short_name(url), Some(format!("connect: {e}"))),
    };
    if !resp.status().is_success() {
        return (
            vec![],
            short_name(url),
            Some(format!("HTTP {}", resp.status().as_u16())),
        );
    }
    let text = match resp.text() {
        Ok(t) => t,
        Err(e) => return (vec![], short_name(url), Some(format!("read: {e}"))),
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return (vec![], short_name(url), Some(format!("parse: {e}"))),
    };
    if let Some(err) = json.get("error") {
        let msg = err["message"].as_str().unwrap_or("API error").to_owned();
        return (vec![], short_name(url), Some(msg));
    }
    // Service name
    let name = json["serviceDescription"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| json["name"].as_str())
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|| short_name(url));

    // Layer list — may be at top level or nested
    let layers_json = json["layers"].as_array().cloned().unwrap_or_default();

    // If no layers listed but the service itself is a Feature Layer (single-layer service)
    let layers: Vec<ArcGisLayerDef> = if layers_json.is_empty() {
        // Single-layer service: treat as layer 0
        let layer_name = json["name"].as_str().unwrap_or("Layer 0").to_owned();
        let geom = json["geometryType"]
            .as_str()
            .unwrap_or("esriGeometryPoint")
            .to_owned();
        vec![ArcGisLayerDef {
            id: 0,
            name: layer_name,
            geometry_type: geom,
            color: egui::Color32::GRAY,
        }]
    } else {
        layers_json
            .iter()
            .filter_map(|l| {
                let id = l["id"].as_u64()? as u32;
                let name = l["name"].as_str().unwrap_or("Layer").to_owned();
                let geom = l["geometryType"]
                    .as_str()
                    .unwrap_or("esriGeometryPoint")
                    .to_owned();
                // Skip tables
                if l["type"].as_str() == Some("Table") {
                    return None;
                }
                Some(ArcGisLayerDef {
                    id,
                    name,
                    geometry_type: geom,
                    color: egui::Color32::GRAY,
                })
            })
            .collect()
    };

    (layers, name, None)
}

/// Apply colors based on source cache's color_offset.
fn apply_layer_colors(layers: &mut Vec<ArcGisLayerDef>, color_offset: usize) {
    for (i, layer) in layers.iter_mut().enumerate() {
        layer.color = palette_color(color_offset + i);
    }
}

/// Poll all enabled layers for all registered sources.
/// Spawns background fetches for stale data; returns immediately with cached features.
pub fn poll(
    source_refs: &[(String, HashSet<u32>)], // (canonical_url, enabled_layer_ids)
    ctx: egui::Context,
) -> Vec<ArcGisFeature> {
    for (url, enabled) in source_refs {
        // Apply colors if layers just became available
        {
            let Ok(mut c) = cache().lock() else { continue };
            if let Some(src) = c.get_mut(url.as_str()) {
                let offset = src.color_offset;
                if let Some(layers) = src.layers.as_mut() {
                    // Apply colors only if they haven't been set yet
                    if layers
                        .first()
                        .map(|l| l.color == egui::Color32::GRAY)
                        .unwrap_or(false)
                    {
                        apply_layer_colors(layers, offset);
                    }
                }
            }
        }

        for layer_id in enabled {
            let should_fetch = {
                let Ok(c) = cache().lock() else { continue };
                let Some(src) = c.get(url.as_str()) else {
                    continue;
                };
                if src.discovering || src.layers.is_none() {
                    continue; // not ready yet
                }
                let entry = src.layer_entries.get(layer_id);
                match entry {
                    None => true,
                    Some(e) => {
                        if e.loading {
                            false
                        } else {
                            e.last_poll
                                .map(|t| t.elapsed() >= POLL_INTERVAL)
                                .unwrap_or(true)
                        }
                    }
                }
            };

            if should_fetch {
                if let Ok(mut c) = cache().lock() {
                    if let Some(src) = c.get_mut(url.as_str()) {
                        let entry =
                            src.layer_entries
                                .entry(*layer_id)
                                .or_insert_with(|| LayerEntry {
                                    features: Vec::new(),
                                    last_poll: None,
                                    loading: false,
                                    status: "idle".into(),
                                });
                        entry.loading = true;
                        entry.status = "loading\u{2026}".into();
                    }
                }
                let url2 = url.clone();
                let lid = *layer_id;
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let (feats, status) = fetch_layer(&url2, lid);
                    if let Ok(mut c) = cache().lock() {
                        if let Some(src) = c.get_mut(&url2) {
                            let entry =
                                src.layer_entries.entry(lid).or_insert_with(|| LayerEntry {
                                    features: Vec::new(),
                                    last_poll: None,
                                    loading: false,
                                    status: "idle".into(),
                                });
                            entry.features = feats;
                            entry.loading = false;
                            entry.last_poll = Some(Instant::now());
                            entry.status = status;
                        }
                    }
                    ctx2.request_repaint();
                });
            }
        }
    }

    // Collect all cached features for enabled layers
    let Ok(c) = cache().lock() else {
        return Vec::new();
    };
    source_refs
        .iter()
        .flat_map(|(url, enabled)| {
            let src = c.get(url.as_str())?;
            let features: Vec<ArcGisFeature> = enabled
                .iter()
                .flat_map(|lid| {
                    src.layer_entries
                        .get(lid)
                        .map(|e| e.features.clone())
                        .unwrap_or_default()
                })
                .collect();
            Some(features)
        })
        .flatten()
        .collect()
}

fn fetch_layer(url: &str, layer_id: u32) -> (Vec<ArcGisFeature>, String) {
    let req_url = format!(
        "{url}/{layer_id}/query?where=1%3D1&outFields=*&returnGeometry=true&outSR=4326&f=json"
    );
    let resp = match http_client().get(&req_url).send() {
        Ok(r) => r,
        Err(e) => return (vec![], format!("connect: {e}")),
    };
    if !resp.status().is_success() {
        return (vec![], format!("HTTP {}", resp.status().as_u16()));
    }
    let text = match resp.text() {
        Ok(t) => t,
        Err(e) => return (vec![], format!("read: {e}")),
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return (vec![], format!("parse: {e}")),
    };
    if json.get("error").is_some() {
        let msg = json["error"]["message"]
            .as_str()
            .unwrap_or("API error")
            .to_owned();
        return (vec![], format!("error: {msg}"));
    }
    let features_json = match json["features"].as_array() {
        Some(f) => f,
        None => return (vec![], "0 features".into()),
    };
    let features: Vec<ArcGisFeature> = features_json
        .iter()
        .filter_map(|f| parse_feature(f, url, layer_id))
        .collect();
    let status = format!("{} features", features.len());
    (features, status)
}

fn parse_feature(
    feat: &serde_json::Value,
    source_url: &str,
    layer_id: u32,
) -> Option<ArcGisFeature> {
    let geom = feat.get("geometry")?;
    let lon = geom["x"].as_f64()? as f32;
    let lat = geom["y"].as_f64()? as f32;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    let attr = feat.get("attributes")?;
    let object_id = attr["OBJECTID"].as_i64().unwrap_or(0);
    let date_ms = attr["Date"]
        .as_i64()
        .or_else(|| attr["date"].as_i64())
        .or_else(|| attr["DATE_"].as_i64());

    let attributes: Vec<(String, String)> = attr
        .as_object()?
        .iter()
        .filter(|(k, _)| {
            k.as_str() != "OBJECTID" && k.as_str() != "Shape__Area" && k.as_str() != "Shape__Length"
        })
        .filter_map(|(k, v)| {
            let s = match v {
                serde_json::Value::String(s) => {
                    let trimmed = s.trim();
                    if trimmed.is_empty() || trimmed == " " {
                        return None;
                    }
                    trimmed.to_owned()
                }
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some((k.clone(), s))
        })
        .collect();

    Some(ArcGisFeature {
        object_id,
        source_url: source_url.to_owned(),
        layer_id,
        location: GeoPoint { lat, lon },
        attributes,
        date_ms,
    })
}

/// Snapshot of a source for UI display.
#[derive(Clone)]
pub struct SourceSnapshot {
    pub url: String,
    pub display_name: String,
    pub discovering: bool,
    pub discover_error: Option<String>,
    pub layers: Option<Vec<ArcGisLayerDef>>,
    pub color_offset: usize,
    /// Per-layer status string.
    pub layer_status: HashMap<u32, String>,
}

pub fn source_snapshot(url: &str) -> Option<SourceSnapshot> {
    let canonical = normalize_url(url);
    let c = cache().lock().ok()?;
    let src = c.get(&canonical)?;
    Some(SourceSnapshot {
        url: canonical.clone(),
        display_name: src.display_name.clone(),
        discovering: src.discovering,
        discover_error: src.discover_error.clone(),
        layers: src.layers.clone(),
        color_offset: src.color_offset,
        layer_status: src
            .layer_entries
            .iter()
            .map(|(id, e)| (*id, e.status.clone()))
            .collect(),
    })
}

pub fn feature_color(feat: &ArcGisFeature) -> egui::Color32 {
    let c = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(src) = c.get(&feat.source_url) {
        let offset = src.color_offset;
        if let Some(layers) = &src.layers {
            if let Some(layer) = layers.iter().find(|l| l.id == feat.layer_id) {
                if layer.color != egui::Color32::GRAY {
                    return layer.color;
                }
                // Color not applied yet — use palette directly
                let li = layers
                    .iter()
                    .position(|l| l.id == feat.layer_id)
                    .unwrap_or(0);
                return palette_color(offset + li);
            }
        }
        return palette_color(offset);
    }
    egui::Color32::GRAY
}

/// Best-effort name from URL (last path segment).
fn short_name(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Unknown")
        .replace('_', " ")
}

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
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
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
