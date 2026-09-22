//! Cache-first public source for the TeleGeography submarine cable map.
//!
//! Submarine cables and their landing points are the physical substrate of the
//! internet, which makes them standing context for almost any event on the
//! globe. TeleGeography publishes both as plain GeoJSON, so this module only
//! owns fetching, caching, and refresh scheduling — parsing and rendering reuse
//! the shared uploaded-layer pipeline in `model::GeoJsonLayer`.
//!
//! All HTTP and disk work happens on a background thread; `tick` never blocks
//! the UI thread.

use crate::model::{AppModel, GeoJsonLayer};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const CABLE_CACHE_FILE: &str = "submarine_cables.json";
const LANDING_CACHE_FILE: &str = "submarine_cable_landings.json";

const CABLE_ENDPOINT: &str = "https://www.submarinecablemap.com/api/v3/cable/cable-geo.json";
const LANDING_ENDPOINT: &str =
    "https://www.submarinecablemap.com/api/v3/landing-point/landing-point-geo.json";

/// Cable routes change on the scale of months, so a daily refresh is plenty and
/// keeps the app light on a free public endpoint.
const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const INITIAL_BACKOFF: Duration = Duration::from_secs(5 * 60);
const MAX_BACKOFF: Duration = Duration::from_secs(2 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Guards against a truncated or error-page response replacing a good cache.
/// The live feeds carry ~729 cables and ~1,925 landing points.
const MIN_CABLE_FEATURES: usize = 100;
const MIN_LANDING_FEATURES: usize = 100;

/// Identify the app to the public endpoint rather than sending a bare default.
const USER_AGENT: &str = concat!(
    "1kEE/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/MLTQ/1kEE)"
);

pub const CABLE_LAYER_NAME: &str = "Submarine cables";
pub const LANDING_LAYER_NAME: &str = "Cable landing points";

/// Human-facing provenance for the layer drawer.
pub const PROJECT_URL: &str = "https://www.submarinecablemap.com/";
pub const ATTRIBUTION: &str = "Submarine cable data © TeleGeography";

/// Landing points share the cable palette's cool end so the two read as one
/// dataset while staying distinguishable. Cables mostly override this with
/// their own per-cable colour from the source.
const CABLE_LAYER_COLOR: [u8; 4] = [90, 170, 255, 200];
const LANDING_LAYER_COLOR: [u8; 4] = [255, 220, 50, 210];

enum WorkerOutcome {
    Loaded {
        generation: u64,
        layers: Vec<GeoJsonLayer>,
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
    loaded_once: bool,
}

impl Default for SourceState {
    fn default() -> Self {
        Self {
            cache_dir: None,
            generation: 0,
            worker: None,
            next_attempt: None,
            failures: 0,
            loaded_once: false,
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

/// Advance the submarine-cable loading lifecycle. Called every frame from
/// `app.rs`; cheap unless a worker result is ready to apply.
///
/// The layer is only fetched once the operator switches it on, so a session
/// that never opens it does no network or disk work at all.
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

        // Repointing the data root invalidates what we loaded from the old one.
        if state.cache_dir != cache_dir {
            state.cache_dir = cache_dir.clone();
            state.loaded_once = false;
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
                    model.submarine_cable_status = "worker stopped unexpectedly".into();
                }
            }
        }

        // Only work when the operator has asked to see the layer.
        let wanted = model.show_submarine_cables;
        let due = state
            .next_attempt
            .map(|at| Instant::now() >= at)
            .unwrap_or(true);

        if wanted && state.worker.is_none() && due && !state.loaded_once {
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
    model.submarine_cable_status = "loading…".into();

    let spawned = thread::Builder::new()
        .name("submarine-cables".into())
        .spawn(move || {
            let outcome = load(generation, cache_dir.as_deref());
            let _ = sender.send(outcome);
        });

    match spawned {
        Ok(_) => {
            source_state().lock().unwrap().worker = Some(receiver);
        }
        Err(error) => {
            let mut state = source_state().lock().unwrap();
            state.failures = state.failures.saturating_add(1);
            state.next_attempt = Some(Instant::now() + backoff(state.failures));
            model.submarine_cable_status = format!("worker spawn failed: {error}");
        }
    }
}

/// Load both layers: a fresh cache first, then the public endpoints, then a
/// stale cache as a last resort so the layer still works offline.
fn load(generation: u64, cache_dir: Option<&Path>) -> WorkerOutcome {
    if let Some((cables, landings)) =
        cache_dir.and_then(|dir| read_cache(dir, Some(REFRESH_INTERVAL)))
    {
        return WorkerOutcome::Loaded {
            generation,
            layers: vec![cables, landings],
            from_cache: true,
            cache_warning: None,
        };
    }

    // Any network path below can still fall back to whatever is on disk.
    let stale = || {
        cache_dir
            .and_then(|dir| read_cache(dir, None))
            .map(|(cables, landings)| WorkerOutcome::Loaded {
                generation,
                layers: vec![cables, landings],
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

    let fetched = fetch(&client, CABLE_ENDPOINT)
        .and_then(|cable_body| {
            let landing_body = fetch(&client, LANDING_ENDPOINT)?;
            Ok((cable_body, landing_body))
        })
        .and_then(|(cable_body, landing_body)| {
            let cables = build_layer(
                CABLE_LAYER_NAME,
                &cable_body,
                CABLE_LAYER_COLOR,
                MIN_CABLE_FEATURES,
            )?;
            let landings = build_layer(
                LANDING_LAYER_NAME,
                &landing_body,
                LANDING_LAYER_COLOR,
                MIN_LANDING_FEATURES,
            )?;
            Ok((cables, landings, cable_body, landing_body))
        });

    let (cables, landings, cable_body, landing_body) = match fetched {
        Ok(parts) => parts,
        Err(error) => {
            return stale().unwrap_or(WorkerOutcome::Failed {
                generation,
                message: error,
            });
        }
    };

    // Only persist once both payloads parsed, so a half-written pair can never
    // be read back as a complete snapshot.
    let cache_warning = cache_dir.and_then(|dir| {
        write_cache(dir, &cable_body, &landing_body)
            .err()
            .map(|error| format!("cache write failed: {error}"))
    });

    WorkerOutcome::Loaded {
        generation,
        layers: vec![cables, landings],
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
        return Err(format!("{} returned HTTP {}", url, response.status()));
    }
    response
        .text()
        .map_err(|error| format!("response read failed: {error}"))
}

/// Parse one GeoJSON payload into a built-in overlay layer.
///
/// Built-in layers start hidden behind their own toggle and unlabelled: the
/// globe view has no viewport culling for labels, and 729 cable names drawn at
/// once is unreadable. The names stay on the features for the layer drawer's
/// label toggle and any future hit-testing.
fn build_layer(
    name: &str,
    body: &str,
    color: [u8; 4],
    min_features: usize,
) -> Result<GeoJsonLayer, String> {
    let mut layer =
        GeoJsonLayer::parse(name.to_owned(), body).map_err(|error| format!("{name}: {error}"))?;
    if layer.features.len() < min_features {
        return Err(format!(
            "{name}: only {} features in response; refusing to trust it",
            layer.features.len()
        ));
    }
    layer.color = color;
    layer.visible = true;
    layer.show_labels = false;
    Ok(layer)
}

/// Resolve the cache directory the same way `deflock_source` does: an explicit
/// configured data root wins, otherwise `Data/` under the selected asset root.
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

fn cache_paths(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join(CABLE_CACHE_FILE), dir.join(LANDING_CACHE_FILE))
}

/// Read both cached payloads. `max_age` of `Some(_)` rejects a cache older than
/// that bound; `None` accepts any age, which is how the offline fallback reuses
/// a stale snapshot. Any problem returns `None` so the caller refetches rather
/// than showing half a dataset.
fn read_cache(dir: &Path, max_age: Option<Duration>) -> Option<(GeoJsonLayer, GeoJsonLayer)> {
    let (cable_path, landing_path) = cache_paths(dir);

    if let Some(max_age) = max_age {
        let cable_age = fs::metadata(&cable_path)
            .ok()?
            .modified()
            .ok()?
            .elapsed()
            .ok()?;
        let landing_age = fs::metadata(&landing_path)
            .ok()?
            .modified()
            .ok()?
            .elapsed()
            .ok()?;
        if cable_age > max_age || landing_age > max_age {
            return None;
        }
    }

    let cable_body = fs::read_to_string(&cable_path).ok()?;
    let landing_body = fs::read_to_string(&landing_path).ok()?;

    let cables = build_layer(
        CABLE_LAYER_NAME,
        &cable_body,
        CABLE_LAYER_COLOR,
        MIN_CABLE_FEATURES,
    )
    .ok()?;
    let landings = build_layer(
        LANDING_LAYER_NAME,
        &landing_body,
        LANDING_LAYER_COLOR,
        MIN_LANDING_FEATURES,
    )
    .ok()?;

    Some((cables, landings))
}

/// Persist both payloads, writing each through a temporary file so an
/// interrupted write cannot leave a truncated cache behind.
fn write_cache(dir: &Path, cable_body: &str, landing_body: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let (cable_path, landing_path) = cache_paths(dir);
    write_atomic(&cable_path, cable_body)?;
    write_atomic(&landing_path, landing_body)?;
    Ok(())
}

fn write_atomic(path: &Path, body: &str) -> Result<(), String> {
    let temp = path.with_extension("tmp");
    fs::write(&temp, body).map_err(|error| error.to_string())?;
    fs::rename(&temp, path).map_err(|error| error.to_string())
}

fn apply_outcome(model: &mut AppModel, outcome: WorkerOutcome) {
    let current = source_state().lock().unwrap().generation;
    match outcome {
        WorkerOutcome::Loaded {
            generation,
            layers,
            from_cache,
            cache_warning,
        } => {
            // A result from a superseded request must not overwrite newer data.
            if generation != current {
                return;
            }
            let cables = layers.first().map(|l| l.features.len()).unwrap_or(0);
            let landings = layers.get(1).map(|l| l.features.len()).unwrap_or(0);

            model.replace_submarine_cable_layers(layers);

            let origin = if from_cache { "cache" } else { "TeleGeography" };
            model.submarine_cable_status =
                format!("{cables} cables · {landings} landings ({origin})");
            if let Some(warning) = cache_warning {
                model.push_log(format!("Submarine cables: {warning}"));
            }

            let mut state = source_state().lock().unwrap();
            state.failures = 0;
            state.loaded_once = true;
            state.next_attempt = None;
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

            model.submarine_cable_status = "unavailable".into();
            model.push_log(format!(
                "Submarine cable load failed ({message}); retrying in {} min.",
                retry.as_secs() / 60
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CABLES: &str = r##"{
        "type": "FeatureCollection",
        "features": [
            {
                "type": "Feature",
                "properties": {"id": "a", "name": "Cable A", "color": "#939597"},
                "geometry": {"type": "MultiLineString", "coordinates": [[[0.0, 0.0], [1.0, 1.0]]]}
            },
            {
                "type": "Feature",
                "properties": {"id": "b", "name": "Cable B", "color": "#0af"},
                "geometry": {"type": "MultiLineString", "coordinates": [[[2.0, 2.0], [3.0, 3.0]]]}
            }
        ]
    }"##;

    #[test]
    fn parses_per_cable_colour_and_name() {
        let layer = build_layer(CABLE_LAYER_NAME, SAMPLE_CABLES, CABLE_LAYER_COLOR, 1)
            .expect("sample should parse");
        assert_eq!(layer.features.len(), 2);
        assert_eq!(layer.features[0].label.as_deref(), Some("Cable A"));
        assert_eq!(layer.features[0].color, Some([0x93, 0x95, 0x97, 220]));
        // Three-digit shorthand expands to full bytes.
        assert_eq!(layer.features[1].color, Some([0x00, 0xaa, 0xff, 220]));
    }

    #[test]
    fn built_in_layers_start_unlabelled() {
        let layer = build_layer(CABLE_LAYER_NAME, SAMPLE_CABLES, CABLE_LAYER_COLOR, 1).unwrap();
        assert!(!layer.show_labels);
        assert!(layer.visible);
        assert_eq!(layer.color, CABLE_LAYER_COLOR);
    }

    #[test]
    fn rejects_a_suspiciously_small_response() {
        // A truncated or error-page response must not replace a good snapshot.
        let error = build_layer(CABLE_LAYER_NAME, SAMPLE_CABLES, CABLE_LAYER_COLOR, 100)
            .expect_err("two features should fail a 100-feature floor");
        assert!(
            error.contains("refusing to trust it"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(build_layer(CABLE_LAYER_NAME, "not json", CABLE_LAYER_COLOR, 1).is_err());
    }

    #[test]
    fn backoff_grows_then_saturates() {
        assert_eq!(backoff(1), INITIAL_BACKOFF);
        assert_eq!(backoff(2), INITIAL_BACKOFF * 2);
        assert_eq!(backoff(200), MAX_BACKOFF);
    }
}
