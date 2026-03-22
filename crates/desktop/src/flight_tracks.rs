/// Live ADS-B flight tracking via the OpenSky Network REST API.
///
/// # Architecture
///
/// `poll(center, ctx)` is called once per render frame and returns the cached
/// flight list immediately (never blocks).  A background thread is spawned
/// whenever the cache has expired (every `POLL_INTERVAL`) or when the globe
/// center has drifted far enough to warrant a fresh bounding box.  The thread
/// calls the OpenSky `/states/all` endpoint, parses the state vectors, writes
/// them to the static cache, and calls `ctx.request_repaint()`.
///
/// # Rate limiting
///
/// OpenSky allows **one request per 10 seconds** for anonymous callers.
/// All outbound HTTP calls (state poll + metadata lookups) share a single
/// `RATE_GATE` that serialises them with an 11-second minimum gap.
/// Metadata threads sleep-wait inside the gate so they never pile up.
/// If the server still returns 429 the existing flight list is preserved and
/// a 60-second `backoff_until` prevents further polling.
///
/// # OpenSky API
///
/// ```
/// GET https://opensky-network.org/api/states/all
///     ?lamin={min_lat}&lomin={min_lon}&lamax={max_lat}&lomax={max_lon}
/// ```
/// Returns `{ "time": unix_ts, "states": [[…], …] }`.
/// Each state vector is a fixed-position array; see `parse_state`.

use crate::model::{FlightTrack, GeoPoint};

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ── persistent HTTP client ─────────────────────────────────────────────────────

fn http_client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest blocking client")
    })
}

// ── global rate gate ──────────────────────────────────────────────────────────
// All OpenSky HTTP calls (states/all + metadata) pass through here.
// Background threads sleep-wait so requests naturally queue instead of
// firing concurrently and triggering 429s.

const RATE_GAP: Duration = Duration::from_millis(11_000); // comfortably > 10 s

fn wait_for_rate_gate() {
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let mutex = LAST.get_or_init(|| Mutex::new(None));

    // Atomically read the last-request time and reserve the next slot.
    let sleep_dur = {
        let mut last = mutex.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let wait = last
            .map(|t| {
                let elapsed = now.duration_since(t);
                if elapsed >= RATE_GAP { Duration::ZERO } else { RATE_GAP - elapsed }
            })
            .unwrap_or(Duration::ZERO);
        // Reserve: mark the "virtual now" at now+wait so concurrent waiters
        // queue up sequentially rather than all sleeping for the same gap.
        *last = Some(now + wait);
        wait
    };

    if !sleep_dur.is_zero() {
        std::thread::sleep(sleep_dur);
    }
}

// ── constants ─────────────────────────────────────────────────────────────────

const POLL_INTERVAL: Duration = Duration::from_secs(15);
const BACKOFF_AFTER_429: Duration = Duration::from_secs(60);
const BOX_HALF_DEG: f32 = 15.0;
const RECENTER_THRESHOLD_DEG: f32 = BOX_HALF_DEG * 0.5;
const MAX_FLIGHTS: usize = 2_000;

// ── static poll cache ─────────────────────────────────────────────────────────

struct PollState {
    flights: Vec<FlightTrack>,
    last_poll: Option<Instant>,
    last_center: Option<GeoPoint>,
    /// Set after a 429 to prevent retry storms.
    backoff_until: Option<Instant>,
    loading: bool,
    pub status: String,
}

fn cache() -> &'static Mutex<PollState> {
    static CACHE: OnceLock<Mutex<PollState>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(PollState {
            flights: Vec::new(),
            last_poll: None,
            last_center: None,
            backoff_until: None,
            loading: false,
            status: "idle".into(),
        })
    })
}

// ── public API ────────────────────────────────────────────────────────────────

/// Returns the current cached flight list immediately.
pub fn poll(center: GeoPoint, ctx: egui::Context) -> Vec<FlightTrack> {
    let should_spawn = cache()
        .lock()
        .map(|g| {
            if g.loading {
                return false;
            }
            // Honour the backoff window after a 429.
            if g.backoff_until.map(|t| t > Instant::now()).unwrap_or(false) {
                return false;
            }
            let interval_expired = g.last_poll
                .map(|t| t.elapsed() >= POLL_INTERVAL)
                .unwrap_or(true);
            let drifted = g.last_center.map(|prev| {
                let dlat = (center.lat - prev.lat).abs();
                let dlon = (center.lon - prev.lon).abs()
                    .min(360.0 - (center.lon - prev.lon).abs());
                dlat > RECENTER_THRESHOLD_DEG || dlon > RECENTER_THRESHOLD_DEG
            }).unwrap_or(false);
            interval_expired || drifted
        })
        .unwrap_or(false);

    if should_spawn {
        if let Ok(mut g) = cache().lock() {
            g.loading = true;
            g.status = "syncing…".into();
        }
        std::thread::spawn(move || {
            match fetch_flights(center) {
                FlightFetchResult::Ok { flights, status } => {
                    if let Ok(mut g) = cache().lock() {
                        g.flights = flights;
                        g.status = status;
                        g.last_poll = Some(Instant::now());
                        g.last_center = Some(center);
                        g.backoff_until = None;
                        g.loading = false;
                    }
                }
                FlightFetchResult::RateLimited => {
                    // Preserve the existing flight list — don't blank the map.
                    if let Ok(mut g) = cache().lock() {
                        let resume = Instant::now() + BACKOFF_AFTER_429;
                        g.status = "rate limited — cooling down 60 s".into();
                        g.backoff_until = Some(resume);
                        g.last_poll = Some(Instant::now());
                        g.loading = false;
                    }
                }
                FlightFetchResult::Error(msg) => {
                    if let Ok(mut g) = cache().lock() {
                        g.status = msg;
                        g.last_poll = Some(Instant::now());
                        g.loading = false;
                    }
                }
            }
            ctx.request_repaint();
        });
    }

    cache()
        .lock()
        .map(|g| g.flights.clone())
        .unwrap_or_default()
}

/// Human-readable polling status for the UI.
pub fn status() -> String {
    cache()
        .lock()
        .map(|g| g.status.clone())
        .unwrap_or_else(|_| "error".into())
}

/// Force a fresh poll on the next call to `poll()`.
#[allow(dead_code)]
pub fn invalidate() {
    if let Ok(mut g) = cache().lock() {
        g.last_poll = None;
        g.last_center = None;
        g.backoff_until = None;
        g.loading = false;
    }
}

// ── HTTP fetch ────────────────────────────────────────────────────────────────

enum FlightFetchResult {
    Ok { flights: Vec<FlightTrack>, status: String },
    RateLimited,
    Error(String),
}

fn fetch_flights(center: GeoPoint) -> FlightFetchResult {
    let min_lat = (center.lat - BOX_HALF_DEG).max(-90.0);
    let max_lat = (center.lat + BOX_HALF_DEG).min(90.0);
    let min_lon = (center.lon - BOX_HALF_DEG).max(-180.0);
    let max_lon = (center.lon + BOX_HALF_DEG).min(180.0);

    let url = format!(
        "https://opensky-network.org/api/states/all\
         ?lamin={min_lat}&lomin={min_lon}&lamax={max_lat}&lomax={max_lon}"
    );

    wait_for_rate_gate();

    let response = match http_client().get(&url).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[flight_tracks] HTTP error: {e}");
            return FlightFetchResult::Error(format!("connect error: {e}"));
        }
    };

    let code = response.status().as_u16();
    if code == 429 {
        eprintln!("[flight_tracks] 429 rate-limited");
        return FlightFetchResult::RateLimited;
    }
    if !response.status().is_success() {
        eprintln!("[flight_tracks] OpenSky returned HTTP {code}");
        return FlightFetchResult::Error(format!("HTTP {code}"));
    }

    let text = match response.text() {
        Ok(t) => t,
        Err(e) => return FlightFetchResult::Error(format!("read error: {e}")),
    };

    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return FlightFetchResult::Error(format!("parse error: {e}")),
    };

    let Some(states) = json["states"].as_array() else {
        return FlightFetchResult::Ok {
            flights: Vec::new(),
            status: format!("0 flights near {:.1}°, {:.1}°", center.lat, center.lon),
        };
    };

    let mut flights: Vec<FlightTrack> = states
        .iter()
        .filter_map(parse_state)
        .take(MAX_FLIGHTS)
        .collect();

    flights.retain(|f| !f.on_ground);

    let status = if flights.is_empty() {
        format!("0 airborne near {:.1}°, {:.1}°", center.lat, center.lon)
    } else {
        format!("{} airborne", flights.len())
    };

    FlightFetchResult::Ok { flights, status }
}

// ── OpenSky state vector parsing ──────────────────────────────────────────────
//
//  [0]  icao24          string
//  [1]  callsign        string | null
//  [2]  origin_country  string
//  [3]  time_position   int | null
//  [4]  last_contact    int
//  [5]  longitude       float | null
//  [6]  latitude        float | null
//  [7]  baro_altitude   float | null  (metres)
//  [8]  on_ground       bool
//  [9]  velocity        float | null  (m/s ground speed)
//  [10] true_track      float | null  (degrees, clockwise from north)
//  [11] vertical_rate   float | null  (m/s)
//  [12] sensors         array | null
//  [13] geo_altitude    float | null  (metres)
//  [14] squawk          string | null
//  [15] spi             bool
//  [16] position_source int

fn parse_state(state: &serde_json::Value) -> Option<FlightTrack> {
    let arr = state.as_array()?;

    let icao24 = arr.get(0)?.as_str()?.trim().to_owned();
    if icao24.is_empty() {
        return None;
    }

    let lat = arr.get(6)?.as_f64()? as f32;
    let lon = arr.get(5)?.as_f64()? as f32;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }

    let callsign = arr.get(1)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let baro_altitude_m = arr.get(7).and_then(|v| v.as_f64()).map(|v| v as f32);
    let on_ground = arr.get(8).and_then(|v| v.as_bool()).unwrap_or(false);

    let speed_knots = arr.get(9)
        .and_then(|v| v.as_f64())
        .map(|v| (v * 1.943_84) as f32);

    let heading_deg = arr.get(10)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let vertical_rate_fpm = arr.get(11)
        .and_then(|v| v.as_f64())
        .map(|v| (v * 196.85) as f32);

    let origin_country = arr.get(2)
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());

    Some(FlightTrack {
        icao24,
        callsign,
        origin_country,
        location: GeoPoint { lat, lon },
        baro_altitude_m,
        on_ground,
        speed_knots,
        heading_deg,
        vertical_rate_fpm,
    })
}

// ── Aircraft metadata (OpenSky extended database) ─────────────────────────────
//
// GET https://opensky-network.org/api/metadata/aircraft/icao/{icao24}
// Fetched lazily on first click, cached indefinitely (static aircraft data).
// Uses the same rate gate as the state poll so clicks never crowd out polls.

/// Static aircraft metadata from the OpenSky extended database.
#[derive(Clone, Debug)]
pub struct AircraftMeta {
    pub registration:      Option<String>,
    pub manufacturer:      Option<String>,
    pub model:             Option<String>,
    pub typecode:          Option<String>,
    pub owner:             Option<String>,
    pub operator:          Option<String>,
    pub operator_callsign: Option<String>,
    pub operator_icao:     Option<String>,
}

enum MetaEntry {
    Loading,
    Loaded(AircraftMeta),
    NotFound,
}

fn meta_cache() -> &'static Mutex<HashMap<String, MetaEntry>> {
    static META: OnceLock<Mutex<HashMap<String, MetaEntry>>> = OnceLock::new();
    META.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Kick off a background metadata fetch for `icao24` if not already cached.
pub fn request_metadata(icao24: &str, ctx: egui::Context) {
    if let Ok(c) = meta_cache().lock() {
        if c.contains_key(icao24) {
            return;
        }
    }
    if let Ok(mut c) = meta_cache().lock() {
        c.insert(icao24.to_owned(), MetaEntry::Loading);
    }
    let icao24 = icao24.to_owned();
    std::thread::spawn(move || {
        // Waits behind any in-progress poll — never races for the quota.
        let entry = fetch_aircraft_meta(&icao24);
        if let Ok(mut c) = meta_cache().lock() {
            // On 429: remove the Loading entry so the user can retry by
            // clicking again rather than being stuck on a spinner forever.
            match entry {
                MetaEntry::Loading => { c.remove(&icao24); }
                other              => { c.insert(icao24, other); }
            }
        }
        ctx.request_repaint();
    });
}

/// Returns cached metadata, or `None` if not yet loaded / not found.
pub fn get_metadata(icao24: &str) -> Option<AircraftMeta> {
    meta_cache().lock().ok().and_then(|c| {
        if let Some(MetaEntry::Loaded(m)) = c.get(icao24) {
            Some(m.clone())
        } else {
            None
        }
    })
}

/// Returns `true` while a metadata fetch is in flight.
pub fn is_meta_loading(icao24: &str) -> bool {
    meta_cache()
        .lock()
        .map(|c| matches!(c.get(icao24), Some(MetaEntry::Loading)))
        .unwrap_or(false)
}

fn fetch_aircraft_meta(icao24: &str) -> MetaEntry {
    let url = format!(
        "https://opensky-network.org/api/metadata/aircraft/icao/{icao24}"
    );

    wait_for_rate_gate();

    let resp = match http_client().get(&url).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[flight_tracks] metadata fetch error: {e}");
            return MetaEntry::NotFound;
        }
    };

    match resp.status().as_u16() {
        404 => return MetaEntry::NotFound,
        429 => {
            eprintln!("[flight_tracks] metadata 429 — will retry on next click");
            // Return Loading so the caller removes it from the map (see above).
            return MetaEntry::Loading;
        }
        code if code >= 400 => {
            eprintln!("[flight_tracks] metadata HTTP {code}");
            return MetaEntry::NotFound;
        }
        _ => {}
    }

    let text = match resp.text() {
        Ok(t) => t,
        Err(_) => return MetaEntry::NotFound,
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return MetaEntry::NotFound,
    };
    let s = |key: &str| -> Option<String> {
        json[key].as_str().filter(|v| !v.is_empty()).map(|v| v.to_owned())
    };
    MetaEntry::Loaded(AircraftMeta {
        registration:      s("registration"),
        manufacturer:      s("manufacturername"),
        model:             s("model"),
        typecode:          s("typecode"),
        owner:             s("owner"),
        operator:          s("operator"),
        operator_callsign: s("operatorcallsign"),
        operator_icao:     s("operatoricao"),
    })
}
