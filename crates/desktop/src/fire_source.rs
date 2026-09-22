//! Cache-first public source for NASA FIRMS active-fire detections.
//!
//! FIRMS publishes VIIRS thermal anomalies as keyless global CSV snapshots
//! covering the last 24 hours. Two satellites are merged here (Suomi-NPP and
//! NOAA-20) because their orbits interleave, roughly doubling revisit coverage.
//!
//! These are deliberately **not** pushed into the event stream. A global day of
//! VIIRS carries ~60,000 detections, which would bury the Factal/USGS brief and
//! the event store under agricultural burning. They are a map layer instead:
//! ambient context you switch on, in the same spirit as the ALPR overlay.
//!
//! All HTTP and disk work happens on a background thread; `tick` never blocks
//! the UI thread.

use crate::model::{AppModel, GeoPoint};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const CACHE_FILE: &str = "firms_active_fires.csv";

/// The two VIIRS C2 near-real-time global 24-hour products. Both are keyless.
const FIRMS_ENDPOINTS: &[(&str, &str)] = &[
    (
        "Suomi-NPP",
        "https://firms.modaps.eosdis.nasa.gov/data/active_fire/suomi-npp-viirs-c2/csv/SUOMI_VIIRS_C2_Global_24h.csv",
    ),
    (
        "NOAA-20",
        "https://firms.modaps.eosdis.nasa.gov/data/active_fire/noaa-20-viirs-c2/csv/J1_VIIRS_C2_Global_24h.csv",
    ),
];

/// The products are rebuilt a few times an hour; refreshing hourly is a fair
/// use of a free public endpoint and keeps the layer meaningfully live.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);
const INITIAL_BACKOFF: Duration = Duration::from_secs(5 * 60);
const MAX_BACKOFF: Duration = Duration::from_secs(2 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// A global day is ~60k detections across both satellites. Guard against a
/// truncated response replacing a good snapshot.
const MIN_DETECTIONS: usize = 500;

const USER_AGENT: &str = concat!(
    "1kEE/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/MLTQ/1kEE)"
);

pub const PROJECT_URL: &str = "https://firms.modaps.eosdis.nasa.gov/";
pub const ATTRIBUTION: &str = "Active fire data: NASA FIRMS (VIIRS S-NPP / NOAA-20)";

/// Reported detection confidence. FIRMS spells these `low` / `nominal` /
/// `high` for VIIRS products.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireConfidence {
    Low,
    Nominal,
    High,
}

impl FireConfidence {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "l" | "low" => Some(Self::Low),
            "n" | "nominal" => Some(Self::Nominal),
            "h" | "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// One VIIRS thermal anomaly ("fire pixel").
#[derive(Clone, Debug, PartialEq)]
pub struct FireDetection {
    pub location: GeoPoint,
    /// Fire radiative power in megawatts — the useful intensity measure.
    pub frp_mw: f32,
    pub confidence: FireConfidence,
    /// Acquisition date as reported (`YYYY-MM-DD`).
    pub acquired_date: String,
    /// Acquisition time as reported (`HHMM`, UTC).
    pub acquired_time: String,
    /// Satellite label, for the detail readout.
    pub satellite: &'static str,
}

enum WorkerOutcome {
    Loaded {
        generation: u64,
        detections: Vec<FireDetection>,
        from_cache: bool,
        cache_warning: Option<String>,
    },
    Failed {
        generation: u64,
        message: String,
    },
}

struct SourceState {
    cache_dir: Option<PathBuf>,
    generation: u64,
    worker: Option<Receiver<WorkerOutcome>>,
    next_attempt: Option<Instant>,
    failures: u8,
}

impl Default for SourceState {
    fn default() -> Self {
        Self {
            cache_dir: None,
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

/// Stop applying worker results during app shutdown.
pub fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
}

/// Advance the active-fire loading lifecycle. Called every frame from `app.rs`.
///
/// Nothing is fetched until the operator enables the layer; after that it
/// refreshes on [`REFRESH_INTERVAL`] for as long as it stays on.
pub fn tick(model: &mut AppModel) {
    if SHUTTING_DOWN.load(Ordering::SeqCst) {
        return;
    }

    let cache_dir = Some(cache_dir(model.selected_root.as_deref()));

    let mut finished = None;
    let mut should_spawn = false;
    let generation;

    {
        let mut state = source_state().lock().unwrap();

        if state.cache_dir != cache_dir {
            state.cache_dir = cache_dir.clone();
            state.worker = None;
            state.next_attempt = None;
            state.failures = 0;
        }

        if let Some(receiver) = &state.worker {
            match receiver.try_recv() {
                Ok(outcome) => {
                    state.worker = None;
                    finished = Some(outcome);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    state.worker = None;
                    state.failures = state.failures.saturating_add(1);
                    state.next_attempt = Some(Instant::now() + backoff(state.failures));
                    model.fire_status = "worker stopped unexpectedly".into();
                }
            }
        }

        let due = state
            .next_attempt
            .map(|at| Instant::now() >= at)
            .unwrap_or(true);

        if model.show_active_fires && state.worker.is_none() && due {
            should_spawn = true;
            state.generation = state.generation.wrapping_add(1);
        }
        generation = state.generation;
    }

    if let Some(outcome) = finished {
        apply_outcome(model, outcome);
    }

    if should_spawn {
        spawn_worker(model, generation, cache_dir);
    }
}

fn backoff(failures: u8) -> Duration {
    let scaled = INITIAL_BACKOFF * 2u32.saturating_pow(failures.saturating_sub(1) as u32);
    scaled.min(MAX_BACKOFF)
}

fn spawn_worker(model: &mut AppModel, generation: u64, cache_dir: Option<PathBuf>) {
    let (sender, receiver) = mpsc::channel();
    if model.active_fires.is_empty() {
        model.fire_status = "loading…".into();
    }

    let spawned = thread::Builder::new()
        .name("firms-active-fires".into())
        .spawn(move || {
            let _ = sender.send(load(generation, cache_dir.as_deref()));
        });

    match spawned {
        Ok(_) => {
            source_state().lock().unwrap().worker = Some(receiver);
        }
        Err(error) => {
            let mut state = source_state().lock().unwrap();
            state.failures = state.failures.saturating_add(1);
            state.next_attempt = Some(Instant::now() + backoff(state.failures));
            model.fire_status = format!("worker spawn failed: {error}");
        }
    }
}

/// Load detections: a fresh cache first, then the public products, then a stale
/// cache so the layer still works offline.
fn load(generation: u64, cache_dir: Option<&Path>) -> WorkerOutcome {
    if let Some(detections) = cache_dir.and_then(|dir| read_cache(dir, Some(REFRESH_INTERVAL))) {
        return WorkerOutcome::Loaded {
            generation,
            detections,
            from_cache: true,
            cache_warning: None,
        };
    }

    let stale = || {
        cache_dir
            .and_then(|dir| read_cache(dir, None))
            .map(|detections| WorkerOutcome::Loaded {
                generation,
                detections,
                from_cache: true,
                cache_warning: None,
            })
    };

    let client = match reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return stale().unwrap_or(WorkerOutcome::Failed {
                generation,
                message: format!("HTTP client error: {error}"),
            });
        }
    };

    let mut detections = Vec::new();
    let mut bodies = Vec::new();
    let mut errors = Vec::new();

    for (satellite, url) in FIRMS_ENDPOINTS {
        match fetch(&client, url) {
            Ok(body) => {
                detections.extend(parse_csv(&body, satellite));
                bodies.push(body);
            }
            // One satellite failing still leaves a usable, if thinner, layer.
            Err(error) => errors.push(format!("{satellite}: {error}")),
        }
    }

    if detections.len() < MIN_DETECTIONS {
        let message = if errors.is_empty() {
            format!(
                "only {} detections parsed; refusing to trust it",
                detections.len()
            )
        } else {
            errors.join("; ")
        };
        return stale().unwrap_or(WorkerOutcome::Failed {
            generation,
            message,
        });
    }

    let cache_warning = cache_dir.and_then(|dir| {
        write_cache(dir, &bodies)
            .err()
            .map(|error| format!("cache write failed: {error}"))
    });

    WorkerOutcome::Loaded {
        generation,
        detections,
        from_cache: false,
        cache_warning,
    }
}

fn fetch(client: &reqwest::blocking::Client, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .map_err(|error| format!("request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    response
        .text()
        .map_err(|error| format!("response read failed: {error}"))
}

/// Parse a FIRMS VIIRS CSV product.
///
/// Columns are resolved by header name rather than position, because the MODIS
/// and VIIRS products order them differently and FIRMS has added columns before.
/// Low-confidence detections are dropped here: they are the noisiest tier
/// (sun glint, hot soil, gas flares) and carry the least analytic value.
fn parse_csv(body: &str, satellite: &'static str) -> Vec<FireDetection> {
    let mut lines = body.lines();
    let Some(header) = lines.next() else {
        return Vec::new();
    };

    let index_of = |want: &str| {
        header
            .split(',')
            .position(|column| column.trim().eq_ignore_ascii_case(want))
    };
    let (Some(lat_idx), Some(lon_idx)) = (index_of("latitude"), index_of("longitude")) else {
        return Vec::new();
    };
    let frp_idx = index_of("frp");
    let confidence_idx = index_of("confidence");
    let date_idx = index_of("acq_date");
    let time_idx = index_of("acq_time");

    let mut out = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        let field = |idx: Option<usize>| idx.and_then(|i| fields.get(i)).map(|s| s.trim());

        let (Some(lat), Some(lon)) = (
            fields
                .get(lat_idx)
                .and_then(|v| v.trim().parse::<f32>().ok()),
            fields
                .get(lon_idx)
                .and_then(|v| v.trim().parse::<f32>().ok()),
        ) else {
            continue;
        };
        if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) {
            continue;
        }

        let confidence = field(confidence_idx)
            .and_then(FireConfidence::parse)
            .unwrap_or(FireConfidence::Nominal);
        if confidence == FireConfidence::Low {
            continue;
        }

        out.push(FireDetection {
            location: GeoPoint { lat, lon },
            frp_mw: field(frp_idx)
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(0.0),
            confidence,
            acquired_date: field(date_idx).unwrap_or_default().to_owned(),
            acquired_time: field(time_idx).unwrap_or_default().to_owned(),
            satellite,
        });
    }
    out
}

/// Resolve the cache directory the same way `deflock_source` does.
fn cache_dir(selected_root: Option<&Path>) -> PathBuf {
    if let Some(data_root) = crate::settings_store::configured_data_root() {
        return data_root;
    }
    selected_root
        .map(Path::to_path_buf)
        .or_else(crate::settings_store::effective_asset_root)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Data")
}

/// Read the cached snapshot. `max_age` of `None` accepts any age, which is how
/// the offline fallback reuses a stale snapshot.
fn read_cache(dir: &Path, max_age: Option<Duration>) -> Option<Vec<FireDetection>> {
    let path = dir.join(CACHE_FILE);
    if let Some(max_age) = max_age {
        let age = fs::metadata(&path).ok()?.modified().ok()?.elapsed().ok()?;
        if age > max_age {
            return None;
        }
    }
    let body = fs::read_to_string(&path).ok()?;
    // The cache concatenates both products, each keeping its own header row, so
    // it round-trips through the same parser one block at a time.
    let detections: Vec<_> = body
        .split("\nlatitude,")
        .enumerate()
        .flat_map(|(idx, block)| {
            let block = if idx == 0 {
                block.to_owned()
            } else {
                format!("latitude,{block}")
            };
            parse_csv(&block, "FIRMS")
        })
        .collect();
    (detections.len() >= MIN_DETECTIONS).then_some(detections)
}

fn write_cache(dir: &Path, bodies: &[String]) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let path = dir.join(CACHE_FILE);
    let temp = path.with_extension("tmp");
    fs::write(&temp, bodies.join("\n")).map_err(|error| error.to_string())?;
    fs::rename(&temp, &path).map_err(|error| error.to_string())
}

fn apply_outcome(model: &mut AppModel, outcome: WorkerOutcome) {
    let current = source_state().lock().unwrap().generation;
    match outcome {
        WorkerOutcome::Loaded {
            generation,
            detections,
            from_cache,
            cache_warning,
        } => {
            if generation != current {
                return;
            }
            let count = detections.len();
            let high = detections
                .iter()
                .filter(|d| d.confidence == FireConfidence::High)
                .count();
            model.replace_active_fires(detections);

            let origin = if from_cache { "cache" } else { "FIRMS" };
            model.fire_status = format!("{count} detections · {high} high confidence ({origin})");
            if let Some(warning) = cache_warning {
                model.push_log(format!("Active fires: {warning}"));
            }

            let mut state = source_state().lock().unwrap();
            state.failures = 0;
            state.next_attempt = Some(Instant::now() + REFRESH_INTERVAL);
        }
        WorkerOutcome::Failed {
            generation,
            message,
        } => {
            if generation != current {
                return;
            }
            let mut state = source_state().lock().unwrap();
            state.failures = state.failures.saturating_add(1);
            let retry = backoff(state.failures);
            state.next_attempt = Some(Instant::now() + retry);
            drop(state);

            model.fire_status = "unavailable".into();
            model.push_log(format!(
                "Active fire load failed ({message}); retrying in {} min.",
                retry.as_secs() / 60
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "latitude,longitude,bright_ti4,scan,track,acq_date,acq_time,satellite,confidence,version,bright_ti5,frp,daynight\n\
-24.25844,21.3893,299.31,0.47,0.64,2026-09-21,0041,N,nominal,2.0NRT,268.39,1.38,N\n\
60.5,-110.25,330.1,0.4,0.6,2026-09-21,1830,N,high,2.0NRT,290.0,145.7,D\n\
10.0,20.0,300.0,0.4,0.6,2026-09-21,0100,N,low,2.0NRT,280.0,3.0,N\n";

    #[test]
    fn parses_fields_by_header_name() {
        let parsed = parse_csv(SAMPLE, "Suomi-NPP");
        // The low-confidence row is dropped.
        assert_eq!(parsed.len(), 2);

        let first = &parsed[0];
        assert!((first.location.lat - -24.25844).abs() < 1e-4);
        assert!((first.location.lon - 21.3893).abs() < 1e-4);
        assert!((first.frp_mw - 1.38).abs() < 1e-4);
        assert_eq!(first.confidence, FireConfidence::Nominal);
        assert_eq!(first.acquired_date, "2026-09-21");
        assert_eq!(first.acquired_time, "0041");
        assert_eq!(first.satellite, "Suomi-NPP");

        assert_eq!(parsed[1].confidence, FireConfidence::High);
        assert!((parsed[1].frp_mw - 145.7).abs() < 1e-4);
    }

    #[test]
    fn tolerates_reordered_columns() {
        // MODIS products order these differently; resolution is by name.
        let reordered = "frp,confidence,longitude,latitude,acq_date,acq_time\n\
9.5,high,30.0,-5.0,2026-09-21,0200\n";
        let parsed = parse_csv(reordered, "NOAA-20");
        assert_eq!(parsed.len(), 1);
        assert!((parsed[0].location.lat - -5.0).abs() < 1e-4);
        assert!((parsed[0].location.lon - 30.0).abs() < 1e-4);
        assert!((parsed[0].frp_mw - 9.5).abs() < 1e-4);
    }

    #[test]
    fn rejects_unusable_input() {
        assert!(parse_csv("", "X").is_empty());
        assert!(parse_csv("nothing,useful\n1,2\n", "X").is_empty());
        // Out-of-range and unparseable coordinates are skipped, not clamped.
        let bad = "latitude,longitude,frp\n999,0,5\nabc,1,5\n";
        assert!(parse_csv(bad, "X").is_empty());
    }

    #[test]
    fn missing_frp_defaults_to_zero_rather_than_dropping_the_row() {
        let parsed = parse_csv("latitude,longitude,confidence\n1.0,2.0,high\n", "X");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].frp_mw, 0.0);
    }
}
