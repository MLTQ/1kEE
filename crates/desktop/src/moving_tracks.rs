/// Live AIS vessel tracking via the AISStream WebSocket API.
///
/// # Architecture
///
/// `poll(api_key, ctx)` is called once per render frame and returns the cached
/// vessel list immediately (never blocks).  A background thread is spawned
/// whenever the cache has expired (every `POLL_INTERVAL`) to open the WebSocket,
/// read messages for `POLL_WINDOW` seconds, close the connection, and write the
/// result back to the static cache before calling `ctx.request_repaint()`.
///
/// # AISStream protocol
///
/// Subscribe with:
/// ```json
/// { "APIKey": "...",
///   "BoundingBoxes": [[[-90,-180],[90,180]]],
///   "FilterMessageTypes": ["PositionReport","ShipStaticData"] }
/// ```
/// Incoming `PositionReport` messages carry lat/lon, true heading, and SOG.
/// Incoming `ShipStaticData` messages carry name, callsign, IMO, destination,
/// ETA, ship type, and draught.  Both reference the same MMSI so they are
/// merged into a single `MovingTrack` record per vessel.

use crate::model::{GeoPoint, MovingTrack};

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ── timing ────────────────────────────────────────────────────────────────────
/// How often to re-connect to AISStream and refresh the vessel cache.
const POLL_INTERVAL: Duration = Duration::from_secs(45);
/// How long to keep the WebSocket open and collect messages each poll.
const POLL_WINDOW: Duration = Duration::from_secs(10);
/// Maximum number of distinct vessels to keep (caps memory at runtime).
const MAX_VESSELS: usize = 1_000;
/// Hard cap on raw messages read per poll cycle to bound thread lifetime.
const MAX_MESSAGES: usize = 5_000;

// ── per-vessel accumulator ────────────────────────────────────────────────────

#[derive(Default)]
struct VesselAccum {
    mmsi: u64,
    name: String,
    lat: Option<f64>,
    lon: Option<f64>,
    heading: Option<f32>,
    speed_knots: Option<f32>,
    callsign: Option<String>,
    imo: Option<u64>,
    destination: Option<String>,
    eta_str: Option<String>,
    ship_type_code: Option<u32>,
    draught_m: Option<f32>,
}

impl VesselAccum {
    fn into_track(self) -> Option<MovingTrack> {
        let lat = self.lat? as f32;
        let lon = self.lon? as f32;
        // Discard vessels outside valid geographic range
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return None;
        }
        let name = if self.name.trim().is_empty() {
            format!("MMSI {}", self.mmsi)
        } else {
            self.name.trim().to_owned()
        };
        Some(MovingTrack {
            mmsi: self.mmsi,
            name,
            location: GeoPoint { lat, lon },
            heading_deg: self.heading,
            speed_knots: self.speed_knots,
            callsign: self.callsign,
            imo: self.imo,
            destination: self.destination,
            eta_str: self.eta_str,
            ship_type_code: self.ship_type_code,
            draught_m: self.draught_m,
        })
    }
}

// ── static cache ──────────────────────────────────────────────────────────────

struct PollState {
    vessels: Vec<MovingTrack>,
    last_poll: Option<Instant>,
    loading: bool,
    pub status: String,
}

fn cache() -> &'static Mutex<PollState> {
    static CACHE: OnceLock<Mutex<PollState>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(PollState {
            vessels: Vec::new(),
            last_poll: None,
            loading: false,
            status: "idle".into(),
        })
    })
}

// ── public API ────────────────────────────────────────────────────────────────

/// Returns the current cached vessel list immediately.
///
/// If no poll has run yet (or the interval has elapsed), spawns a background
/// thread to refresh the cache.  The caller must handle the case where the
/// returned list is empty (poll in progress or key not configured).
pub fn poll(api_key: &str, ctx: egui::Context) -> Vec<MovingTrack> {
    if api_key.is_empty() {
        return Vec::new();
    }

    let should_spawn = cache()
        .lock()
        .map(|g| {
            !g.loading
                && g.last_poll
                    .map(|t| t.elapsed() >= POLL_INTERVAL)
                    .unwrap_or(true)
        })
        .unwrap_or(false);

    if should_spawn {
        if let Ok(mut g) = cache().lock() {
            g.loading = true;
            g.status = "syncing".into();
        }
        let key = api_key.to_owned();
        std::thread::spawn(move || {
            let vessels = fetch_vessels(&key);
            if let Ok(mut g) = cache().lock() {
                g.vessels = vessels;
                g.loading = false;
                g.last_poll = Some(Instant::now());
                g.status = if g.vessels.is_empty() {
                    "no data".into()
                } else {
                    format!("{} vessels", g.vessels.len())
                };
            }
            ctx.request_repaint();
        });
    }

    cache()
        .lock()
        .map(|g| g.vessels.clone())
        .unwrap_or_default()
}

/// Human-readable polling status string for the UI ("idle", "syncing", "N vessels").
pub fn status() -> String {
    cache()
        .lock()
        .map(|g| g.status.clone())
        .unwrap_or_else(|_| "error".into())
}

/// Force a fresh poll on the next call to `poll()`.
pub fn invalidate() {
    if let Ok(mut g) = cache().lock() {
        g.last_poll = None;
        g.loading = false;
    }
}

// ── WebSocket polling ─────────────────────────────────────────────────────────

fn fetch_vessels(api_key: &str) -> Vec<MovingTrack> {
    use tungstenite::{Message, connect};

    let url = "wss://stream.aisstream.io/v0/stream";
    let (mut ws, _) = match connect(url) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("[moving_tracks] AISStream connect error: {e}");
            return Vec::new();
        }
    };

    // Send subscription for global positions + static data.
    let sub = serde_json::json!({
        "APIKey": api_key,
        "BoundingBoxes": [[[-90.0, -180.0], [90.0, 180.0]]],
        "FilterMessageTypes": ["PositionReport", "ShipStaticData"]
    });
    if ws.send(Message::Text(sub.to_string())).is_err() {
        return Vec::new();
    }

    let mut accumulator: HashMap<u64, VesselAccum> = HashMap::new();
    let deadline = Instant::now() + POLL_WINDOW;
    let mut message_count: usize = 0;

    loop {
        if Instant::now() >= deadline {
            break;
        }
        if accumulator.len() >= MAX_VESSELS || message_count >= MAX_MESSAGES {
            break;
        }

        match ws.read() {
            Ok(Message::Text(text)) => {
                message_count += 1;
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                    ingest_message(&val, &mut accumulator);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                eprintln!("[moving_tracks] read error: {e}");
                break;
            }
        }
    }

    let _ = ws.close(None);

    accumulator
        .into_values()
        .filter_map(|v| v.into_track())
        .collect()
}

// ── message ingestion ─────────────────────────────────────────────────────────

fn ingest_message(val: &serde_json::Value, acc: &mut HashMap<u64, VesselAccum>) {
    let Some(msg_type) = val["MessageType"].as_str() else {
        return;
    };
    let meta = &val["MetaData"];
    let Some(mmsi) = meta["MMSI"].as_u64() else {
        return;
    };
    if mmsi == 0 {
        return;
    }

    let entry = acc.entry(mmsi).or_insert_with(|| VesselAccum { mmsi, ..Default::default() });

    // Ship name lives in MetaData on every message type.
    if entry.name.is_empty() {
        if let Some(n) = meta["ShipName"].as_str() {
            let trimmed = n.trim();
            if !trimmed.is_empty() {
                entry.name = trimmed.to_owned();
            }
        }
    }

    match msg_type {
        "PositionReport" => ingest_position(val, entry),
        "ShipStaticData" => ingest_static(val, entry),
        _ => {}
    }
}

fn ingest_position(val: &serde_json::Value, entry: &mut VesselAccum) {
    let pr = &val["Message"]["PositionReport"];

    if let Some(lat) = pr["Latitude"].as_f64() {
        entry.lat = Some(lat);
    }
    if let Some(lon) = pr["Longitude"].as_f64() {
        entry.lon = Some(lon);
    }
    // TrueHeading 511 = unavailable
    if let Some(h) = pr["TrueHeading"].as_f64() {
        if h < 360.0 {
            entry.heading = Some(h as f32);
        }
    }
    // Sog = speed over ground in knots (×10 encoding in raw NMEA but
    // AISStream delivers it already divided)
    if let Some(s) = pr["Sog"].as_f64() {
        entry.speed_knots = Some(s as f32);
    }
}

fn ingest_static(val: &serde_json::Value, entry: &mut VesselAccum) {
    let sd = &val["Message"]["ShipStaticData"];

    if let Some(cs) = sd["CallSign"].as_str() {
        let cs = cs.trim();
        if !cs.is_empty() {
            entry.callsign = Some(cs.to_owned());
        }
    }
    if let Some(imo) = sd["ImoNumber"].as_u64() {
        if imo > 0 {
            entry.imo = Some(imo);
        }
    }
    if let Some(name) = sd["Name"].as_str() {
        let name = name.trim();
        if !name.is_empty() {
            entry.name = name.to_owned();
        }
    }
    if let Some(t) = sd["Type"].as_u64() {
        entry.ship_type_code = Some(t as u32);
    }
    if let Some(d) = sd["Draught"].as_f64() {
        if d > 0.0 {
            entry.draught_m = Some(d as f32);
        }
    }
    if let Some(dest) = sd["Destination"].as_str() {
        let dest = dest.trim();
        if !dest.is_empty() {
            entry.destination = Some(dest.to_owned());
        }
    }
    // Build ETA string from the ETA sub-object: {"Month":M,"Day":D,"Hour":H,"Minute":Mi}
    let eta = &sd["Eta"];
    if !eta.is_null() {
        let month = eta["Month"].as_u64().unwrap_or(0);
        let day = eta["Day"].as_u64().unwrap_or(0);
        let hour = eta["Hour"].as_u64().unwrap_or(24);
        let minute = eta["Minute"].as_u64().unwrap_or(60);
        if month > 0 && day > 0 && hour <= 23 && minute <= 59 {
            entry.eta_str = Some(format!("{month:02}/{day:02} {hour:02}:{minute:02} UTC"));
        }
    }
}
