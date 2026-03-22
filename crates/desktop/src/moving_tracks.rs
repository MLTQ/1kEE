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

/// Half-width of the bounding box sent to AISStream (degrees).
/// ±15° keeps us well within free-tier limits while covering a large region.
const BOX_HALF_DEG: f32 = 15.0;

/// Re-poll immediately when the globe center moves more than this far from
/// the center used for the last poll (degrees, great-circle approximation).
const RECENTER_THRESHOLD_DEG: f32 = BOX_HALF_DEG * 0.5;

// ── static cache ──────────────────────────────────────────────────────────────

struct PollState {
    vessels: Vec<MovingTrack>,
    last_poll: Option<Instant>,
    /// Globe center used for the last successful poll bounding box.
    last_center: Option<GeoPoint>,
    loading: bool,
    pub status: String,
}

fn cache() -> &'static Mutex<PollState> {
    static CACHE: OnceLock<Mutex<PollState>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(PollState {
            vessels: Vec::new(),
            last_poll: None,
            last_center: None,
            loading: false,
            status: "idle".into(),
        })
    })
}

// ── public API ────────────────────────────────────────────────────────────────

/// Returns the current cached vessel list immediately.
///
/// Spawns a background refresh when the poll interval has elapsed **or** when
/// the globe center has drifted more than `RECENTER_THRESHOLD_DEG` outside the
/// previous bounding box — so panning to a new area of the globe triggers a
/// fresh fetch automatically.
pub fn poll(api_key: &str, center: GeoPoint, ctx: egui::Context) -> Vec<MovingTrack> {
    if api_key.is_empty() {
        return Vec::new();
    }

    let should_spawn = cache()
        .lock()
        .map(|g| {
            if g.loading {
                return false;
            }
            // Re-poll on interval expiry.
            let interval_expired = g.last_poll
                .map(|t| t.elapsed() >= POLL_INTERVAL)
                .unwrap_or(true);
            // Re-poll when the globe center has moved significantly outside the
            // previous bounding box so vessels stay relevant to the current view.
            let drifted = g.last_center.map(|prev| {
                let dlat = (center.lat - prev.lat).abs();
                let dlon = (center.lon - prev.lon).abs().min(360.0 - (center.lon - prev.lon).abs());
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
        let key = api_key.to_owned();
        std::thread::spawn(move || {
            let (vessels, status) = fetch_vessels(&key, center);
            if let Ok(mut g) = cache().lock() {
                g.vessels = vessels;
                g.loading = false;
                g.last_poll = Some(Instant::now());
                g.last_center = Some(center);
                g.status = status;
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
        g.last_center = None;
        g.loading = false;
    }
}

// ── WebSocket polling ─────────────────────────────────────────────────────────

/// Returns `(vessels, status_string)`.  Never blocks the caller — all IO
/// happens inside a background thread (see `poll`).
fn fetch_vessels(api_key: &str, center: GeoPoint) -> (Vec<MovingTrack>, String) {
    use tungstenite::{Message, connect};

    let url = "wss://stream.aisstream.io/v0/stream";
    let (mut ws, _) = match connect(url) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("[moving_tracks] AISStream connect error: {e}");
            return (Vec::new(), format!("connect error: {e}"));
        }
    };

    // Set a short read timeout on the underlying TCP stream so the deadline
    // loop can actually fire.  Without this, ws.read() blocks indefinitely on
    // a quiet socket and the deadline check at the top of the loop is never
    // reached.  MaybeTlsStream wraps either a plain or a rustls TcpStream.
    {
        use tungstenite::stream::MaybeTlsStream;
        let timeout = Some(Duration::from_millis(500));
        let result = match ws.get_mut() {
            MaybeTlsStream::Plain(tcp) => tcp.set_read_timeout(timeout),
            MaybeTlsStream::Rustls(tls) => tls.get_ref().set_read_timeout(timeout),
            _ => Ok(()),
        };
        if let Err(e) = result {
            eprintln!("[moving_tracks] set_read_timeout: {e}");
        }
    }

    // Build a ±BOX_HALF_DEG bounding box around the current globe center.
    // This keeps us within free-tier limits and returns relevant local traffic
    // rather than requesting every ship on Earth.
    let min_lat = (center.lat - BOX_HALF_DEG).max(-90.0) as f64;
    let max_lat = (center.lat + BOX_HALF_DEG).min(90.0) as f64;
    let min_lon = (center.lon - BOX_HALF_DEG).max(-180.0) as f64;
    let max_lon = (center.lon + BOX_HALF_DEG).min(180.0) as f64;

    // Send subscription for positions + static data within the local box.
    let sub = serde_json::json!({
        "APIKey": api_key,
        "BoundingBoxes": [[[min_lat, min_lon], [max_lat, max_lon]]],
        "FilterMessageTypes": ["PositionReport", "ShipStaticData"]
    });
    if let Err(e) = ws.send(Message::Text(sub.to_string())) {
        return (Vec::new(), format!("subscribe error: {e}"));
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
            // Timeout / WouldBlock — normal on a quiet socket; loop back and
            // re-check the deadline.
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

    let vessels: Vec<MovingTrack> = accumulator
        .into_values()
        .filter_map(|v| v.into_track())
        .collect();

    let status = if vessels.is_empty() {
        format!(
            "0 vessels near {:.1}°, {:.1}° (check API key)",
            center.lat, center.lon
        )
    } else {
        format!("{} vessels", vessels.len())
    };

    (vessels, status)
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
