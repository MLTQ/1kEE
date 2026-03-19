use crate::model::GeoPoint;
use crate::settings_store;
use osmpbf::{BlobDecode, BlobReader, Element, ElementReader};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// In-memory caches — eliminate per-frame SQLite hits on the render thread
// ---------------------------------------------------------------------------

/// Notes of every job that has ever been queued/completed, loaded once from
/// the DB at startup and updated in-process.  `has_job_note` checks here
/// first so the hot path never opens SQLite.
fn known_notes() -> &'static Mutex<HashSet<String>> {
    static CACHE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// True while at least one job is in 'queued' or 'running' state.
/// Updated in-process; eliminates the per-frame `snapshots()` call in
/// `has_active_jobs`.
fn active_jobs_flag() -> &'static AtomicBool {
    static FLAG: OnceLock<AtomicBool> = OnceLock::new();
    FLAG.get_or_init(|| AtomicBool::new(false))
}

/// Human-readable note for the currently running job (shown in the UI).
fn current_job_note_store() -> &'static Mutex<Option<String>> {
    static STORE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(None))
}

/// Whether the in-memory caches have been hydrated from the DB yet.
fn caches_initialized() -> &'static AtomicBool {
    static INIT: OnceLock<AtomicBool> = OnceLock::new();
    INIT.get_or_init(|| AtomicBool::new(false))
}

/// Load known notes and active-job flag from the DB once at startup.
fn initialize_caches(db_path: &Path) {
    if caches_initialized().swap(true, Ordering::SeqCst) {
        return; // already done
    }
    let Ok(connection) = open_runtime_db(db_path) else { return };

    // Populate known_notes with every note that was ever queued/completed.
    if let Ok(mut stmt) = connection.prepare(
        "SELECT note FROM osm_ingest_jobs \
         WHERE state IN ('queued','running','completed') AND note != ''",
    ) {
        let notes: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .collect();
        if let Ok(mut guard) = known_notes().lock() {
            guard.extend(notes);
        }
    }

    // Set active_jobs_flag if any jobs are still queued/running.
    let active: usize = connection
        .query_row(
            "SELECT COUNT(*) FROM osm_ingest_jobs \
             WHERE state IN ('queued','running')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    active_jobs_flag().store(active > 0, Ordering::Relaxed);
}

/// Returns the human-readable note for the currently running job, if any.
pub fn active_job_note() -> Option<String> {
    current_job_note_store().lock().ok()?.clone()
}

/// Monotonically increasing counter bumped every time a road import job
/// completes.  The road tile cache stores this value and considers itself
/// stale whenever the counter has advanced — ensuring newly imported data
/// is always picked up without requiring a manual toggle.
fn road_data_gen() -> &'static std::sync::atomic::AtomicU64 {
    static GEN: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    GEN.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

pub fn road_data_generation() -> u64 {
    road_data_gen().load(Ordering::Relaxed)
}

fn water_data_gen() -> &'static std::sync::atomic::AtomicU64 {
    static GEN: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    GEN.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

pub fn water_data_generation() -> u64 {
    water_data_gen().load(Ordering::Relaxed)
}

const PLANET_PBF_NAME: &str = "planet-latest.osm.pbf";
const RUNTIME_DB_NAME: &str = "osm_runtime.sqlite";
const PLANET_ROADS_NOTE: &str = "planet_roads_bootstrap_v1";
const FOCUS_ROADS_NOTE_PREFIX: &str = "focus_roads_v1";
const ROAD_TILE_ZOOMS: &[u8] = &[4, 6, 8, 10];
const PROGRESS_FLUSH_INTERVAL: usize = 25_000;
const FOCUS_SCAN_PROGRESS_INTERVAL: usize = 2_000_000;
const FOCUS_NODE_MARGIN_DEGREES: f32 = 0.08;
const DEFAULT_FOCUS_RADIUS_MILES: f32 = 20.0;
/// Sentinel source path used when queuing an Overpass-backed focus job
/// (i.e. no local planet.osm.pbf is available).
const OVERPASS_SOURCE: &str = "overpass";
const OVERPASS_ENDPOINT: &str = "https://overpass-api.de/api/interpreter";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsmFeatureKind {
    Roads,
    Buildings,
    Water,
}

impl OsmFeatureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Roads     => "roads",
            Self::Buildings => "buildings",
            Self::Water     => "water",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GeoBounds {
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
}

pub struct OsmInventory {
    pub planet_path: Option<PathBuf>,
    pub planet_size_bytes: u64,
    pub runtime_db_path: Option<PathBuf>,
    pub runtime_db_ready: bool,
    pub queued_jobs: usize,
    pub road_tiles: usize,
    pub building_tiles: usize,
    pub water_tiles: usize,
    pub primary_runtime_source: &'static str,
}

/// A water feature polyline from OSM.  Waterways (rivers, streams, canals)
/// are open polylines; water bodies (lakes, ponds, reservoirs) are closed
/// (first ≈ last point) and rendered as outlines + light fill.
#[derive(Clone, Debug)]
pub struct WaterPolyline {
    pub way_id: i64,
    pub water_class: String, // "river"|"stream"|"canal"|"drain"|"lake"|"reservoir"
    pub name: Option<String>,
    pub points: Vec<GeoPoint>,
    pub is_area: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoadLayerKind {
    Major,
    Minor,
}

#[derive(Clone, Debug)]
pub struct RoadPolyline {
    pub way_id: i64,
    pub road_class: String,
    pub name: Option<String>,
    pub points: Vec<GeoPoint>,
}

#[derive(Clone)]
pub struct OsmJobSnapshot {
    pub label: String,
    pub state: String,
    pub note: String,
}

#[derive(Clone)]
struct OsmJob {
    id: i64,
    feature_kind: OsmFeatureKind,
    source_path: PathBuf,
    bounds: GeoBounds,
    note: String,
}

struct ActiveWorker {
    handle: JoinHandle<()>,
}

impl OsmInventory {
    pub fn detect_from(selected_root: Option<&Path>) -> Self {
        let planet_path = find_planet_pbf(selected_root);
        let planet_size_bytes = planet_path
            .as_ref()
            .and_then(|path| fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        let runtime_db_path = runtime_db_path(selected_root);

        let (runtime_db_ready, queued_jobs, road_tiles, building_tiles, water_tiles) = runtime_db_path
            .as_ref()
            .filter(|path| path.exists())
            .and_then(|path| read_runtime_counts(path).ok())
            .unwrap_or((false, 0, 0, 0, 0));

        let primary_runtime_source = if road_tiles > 0 || building_tiles > 0 || water_tiles > 0 {
            "Planet OSM -> shared SQLite tile store"
        } else if runtime_db_ready {
            "Planet OSM detected, runtime schema ready"
        } else if planet_path.is_some() {
            "Planet OSM source detected"
        } else {
            "No OSM planet source detected"
        };

        Self {
            planet_path,
            planet_size_bytes,
            runtime_db_path,
            runtime_db_ready,
            queued_jobs,
            road_tiles,
            building_tiles,
            water_tiles,
            primary_runtime_source,
        }
    }

    pub fn status_label(&self) -> &'static str {
        if self.queued_jobs > 0 {
            "building"
        } else if self.runtime_db_ready {
            "ready"
        } else if self.planet_path.is_some() {
            "source"
        } else {
            "missing"
        }
    }

    pub fn status_summary(&self) -> String {
        format!(
            "Planet {} | Runtime DB {} | queued jobs {} | road tiles {} | building tiles {}",
            self.planet_path
                .as_ref()
                .map(|_| human_bytes(self.planet_size_bytes))
                .unwrap_or_else(|| "missing".into()),
            yes_no(self.runtime_db_ready),
            self.queued_jobs,
            self.road_tiles,
            self.building_tiles
        )
    }

    pub fn status_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("OSM assets detected: {}", self.status_summary())];
        lines.push(format!(
            "Preferred OSM runtime source: {}",
            self.primary_runtime_source
        ));

        if let Some(planet_path) = &self.planet_path {
            lines.push(format!(
                "Planet source: {} ({})",
                planet_path.display(),
                human_bytes(self.planet_size_bytes)
            ));
        }

        if let Some(runtime_db_path) = &self.runtime_db_path {
            lines.push(format!("OSM runtime DB: {}", runtime_db_path.display()));
        }

        lines
    }
}

pub fn ensure_runtime_store(selected_root: Option<&Path>) -> Result<PathBuf, String> {
    let db_path = runtime_db_path(selected_root).ok_or_else(|| {
        "Unable to resolve Derived/ root for the shared OSM runtime store.".to_owned()
    })?;
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let connection = open_runtime_db(&db_path).map_err(|error| error.to_string())?;
    ensure_runtime_schema(&connection).map_err(|error| error.to_string())?;

    if let Some(planet_path) = find_planet_pbf(selected_root) {
        register_planet_source(&connection, &planet_path).map_err(|error| error.to_string())?;
    }

    // Hydrate in-memory caches from the DB (no-op after first call).
    initialize_caches(&db_path);

    Ok(db_path)
}

pub fn queue_region_job(
    selected_root: Option<&Path>,
    feature_kind: OsmFeatureKind,
    bounds: GeoBounds,
    priority: i64,
    note: Option<&str>,
) -> Result<(), String> {
    let source_path = find_planet_pbf(selected_root)
        .ok_or_else(|| "No planet-latest.osm.pbf source found for OSM ingest.".to_owned())?;
    let db_path = ensure_runtime_store(selected_root)?;
    let connection = open_runtime_db(&db_path).map_err(|error| error.to_string())?;

    let job_note = note.unwrap_or("");
    let existing_count: usize = connection
        .query_row(
            "SELECT COUNT(*) FROM osm_ingest_jobs
             WHERE feature_kind = ?1 AND note = ?2 AND state IN ('queued', 'running', 'completed')",
            params![feature_kind.as_str(), job_note],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if existing_count > 0 {
        return Ok(());
    }

    let now = unix_timestamp();
    connection
        .execute(
            "INSERT INTO osm_ingest_jobs (
                feature_kind, state, source_path,
                min_lat, max_lat, min_lon, max_lon, priority,
                requested_at_unix, updated_at_unix, note
             ) VALUES (?1, 'queued', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
            params![
                feature_kind.as_str(),
                source_path.display().to_string(),
                bounds.min_lat,
                bounds.max_lat,
                bounds.min_lon,
                bounds.max_lon,
                priority,
                now,
                job_note,
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn queue_planet_roads_import(selected_root: Option<&Path>) -> Result<bool, String> {
    let queued_before = has_job_note(selected_root, PLANET_ROADS_NOTE)?;
    if queued_before {
        return Ok(false);
    }

    queue_region_job(
        selected_root,
        OsmFeatureKind::Roads,
        GeoBounds {
            min_lat: -85.0511,
            max_lat: 85.0511,
            min_lon: -180.0,
            max_lon: 180.0,
        },
        10,
        Some(PLANET_ROADS_NOTE),
    )?;
    Ok(true)
}

pub fn queue_focus_roads_import(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    radius_miles: f32,
) -> Result<bool, String> {
    let radius_miles = radius_miles.clamp(5.0, 150.0);
    let bounds = focus_bounds(focus, radius_miles);
    // Quantise radius to the nearest 5 miles so nearby zoom levels share jobs.
    let radius_bucket = ((radius_miles / 5.0).ceil() as u32) * 5;
    let note = format!(
        "{FOCUS_ROADS_NOTE_PREFIX}_{:.3}_{:.3}_r{}",
        focus.lat, focus.lon, radius_bucket
    );

    // Fast in-memory check — avoids opening SQLite on every frame for
    // areas that have already been loaded.
    if known_notes()
        .lock()
        .map(|g| g.contains(&note))
        .unwrap_or(false)
    {
        return Ok(false);
    }

    // Use osmium + planet file when available, otherwise Overpass.
    let source_path = if find_planet_pbf(selected_root).is_some()
        && !settings_store::prefer_overpass()
    {
        find_planet_pbf(selected_root)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    } else {
        OVERPASS_SOURCE.to_owned()
    };

    let db_path = ensure_runtime_store(selected_root)?;
    let connection = open_runtime_db(&db_path).map_err(|e| e.to_string())?;

    let job_note = &note;
    let existing_count: usize = connection
        .query_row(
            "SELECT COUNT(*) FROM osm_ingest_jobs
             WHERE feature_kind = ?1 AND note = ?2 AND state IN ('queued', 'running', 'completed')",
            params![OsmFeatureKind::Roads.as_str(), job_note],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if existing_count > 0 {
        // Job exists in DB but wasn't in our in-memory cache (e.g. loaded from
        // a previous session).  Add it so future checks are free.
        if let Ok(mut notes) = known_notes().lock() {
            notes.insert(note.clone());
        }
        return Ok(false);
    }

    let now = unix_timestamp();
    connection
        .execute(
            "INSERT INTO osm_ingest_jobs (
                feature_kind, state, source_path,
                min_lat, max_lat, min_lon, max_lon, priority,
                requested_at_unix, updated_at_unix, note
             ) VALUES (?1, 'queued', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
            params![
                OsmFeatureKind::Roads.as_str(),
                source_path,
                bounds.min_lat,
                bounds.max_lat,
                bounds.min_lon,
                bounds.max_lon,
                100_i64,
                now,
                job_note,
            ],
        )
        .map_err(|e| e.to_string())?;

    // Update in-memory caches so tick() and future calls are fast.
    if let Ok(mut notes) = known_notes().lock() {
        notes.insert(note);
    }
    active_jobs_flag().store(true, Ordering::Relaxed);

    Ok(true)
}

pub fn tick(selected_root: Option<&Path>) {
    let worker = worker();
    let mut guard = match worker.lock() {
        Ok(g) => g,
        Err(_) => return,
    };

    if let Some(active) = guard.as_ref() {
        if active.handle.is_finished() {
            // Worker finished — join it and fall through to check for the next job.
            let active = guard.take().expect("finished worker present");
            drop(guard);
            let _ = active.handle.join();
            if let Ok(mut note) = current_job_note_store().lock() {
                *note = None;
            }
        } else {
            return; // worker still running — nothing to do
        }
    } else {
        drop(guard);
        // Fast path: if no jobs are queued, skip SQLite entirely.
        if !active_jobs_flag().load(Ordering::Relaxed) {
            return;
        }
    }

    // Reach here only when: (a) a worker just finished, or (b) the flag says
    // there are queued jobs.  Now open the DB to find the next job.
    let Ok(db_path) = ensure_runtime_store(selected_root) else {
        return;
    };

    // Recover orphaned 'running' rows once, rate-limited to avoid hammering
    // the DB on every tick when there's nothing to do.
    static LAST_RECOVERY: OnceLock<Mutex<Instant>> = OnceLock::new();
    {
        let mut last = LAST_RECOVERY
            .get_or_init(|| Mutex::new(Instant::now() - Duration::from_secs(60)))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if last.elapsed() > Duration::from_secs(30) {
            *last = Instant::now();
            drop(last);
            let _ = recover_orphaned_running_jobs(&db_path);
        }
    }

    let connection = match open_runtime_db(&db_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let Some(job) = fetch_next_job(&connection).ok().flatten() else {
        // No queued jobs — clear the flag so future ticks are free.
        active_jobs_flag().store(false, Ordering::Relaxed);
        if let Ok(mut note) = current_job_note_store().lock() {
            *note = None;
        }
        return;
    };
    drop(connection);

    // Store the job note for the progress bar.
    if let Ok(mut note) = current_job_note_store().lock() {
        *note = Some(job.note.clone());
    }

    let handle = thread::spawn(move || {
        let result = match job.feature_kind {
            OsmFeatureKind::Roads     => import_planet_roads(&db_path, &job),
            OsmFeatureKind::Water     => import_planet_water(&db_path, &job),
            OsmFeatureKind::Buildings => {
                Err("Planet building import is not implemented yet.".to_owned())
            }
        };

        match result {
            Ok(summary) => {
                let _ = mark_job_completed(&db_path, job.id, &summary);
                match job.feature_kind {
                    OsmFeatureKind::Roads => {
                        road_data_gen().fetch_add(1, Ordering::Relaxed);
                    }
                    OsmFeatureKind::Water => {
                        water_data_gen().fetch_add(1, Ordering::Relaxed);
                    }
                    OsmFeatureKind::Buildings => {}
                }
            }
            Err(error) => {
                let _ = mark_job_failed(&db_path, job.id, &error);
            }
        }

        // Signal that this worker is done; tick() will clear the flag if no
        // more jobs are queued.
        crate::app::request_repaint();
    });

    if let Ok(mut guard) = worker.lock() {
        *guard = Some(ActiveWorker { handle });
    }
}

/// O(1) — reads an in-memory AtomicBool, never touches SQLite.
pub fn has_active_jobs(_selected_root: Option<&Path>) -> bool {
    active_jobs_flag().load(Ordering::Relaxed)
}

pub fn snapshots(selected_root: Option<&Path>) -> Vec<OsmJobSnapshot> {
    let Some(db_path) = runtime_db_path(selected_root) else {
        return Vec::new();
    };
    let Ok(connection) = open_runtime_db(&db_path) else {
        return Vec::new();
    };
    let Ok(mut statement) = connection.prepare(
        "SELECT feature_kind, state, note, updated_at_unix
         FROM osm_ingest_jobs
         ORDER BY updated_at_unix DESC, requested_at_unix DESC",
    ) else {
        return Vec::new();
    };

    let rows = match statement.query_map([], |row| {
        let feature_kind: String = row.get(0)?;
        let state: String = row.get(1)?;
        let note: String = row.get(2)?;
        Ok(OsmJobSnapshot {
            label: match feature_kind.as_str() {
                "roads" if note == PLANET_ROADS_NOTE => "Global roads bootstrap".to_owned(),
                "roads" if note.starts_with(FOCUS_ROADS_NOTE_PREFIX) => {
                    "Focused roads import".to_owned()
                }
                "roads" => "Road region import".to_owned(),
                "buildings" => "Building import".to_owned(),
                "water" => "Water feature import".to_owned(),
                _ => "OSM ingest job".to_owned(),
            },
            state,
            note,
        })
    }) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };

    rows.filter_map(Result::ok).collect()
}

pub fn find_planet_pbf(selected_root: Option<&Path>) -> Option<PathBuf> {
    if let Some(configured) = settings_store::configured_planet_path() {
        if configured.exists() {
            return Some(configured);
        }
    }

    if let Some(root) = selected_root {
        if let Some(path) = find_planet_from(root) {
            return Some(path);
        }
    }

    if let Some(asset_root) = settings_store::effective_asset_root() {
        if let Some(path) = find_planet_from(&asset_root) {
            return Some(path);
        }
    }

    None
}

pub fn validate_reader(selected_root: Option<&Path>) -> Result<(), String> {
    let path = find_planet_pbf(selected_root)
        .ok_or_else(|| "No planet-latest.osm.pbf source found for validation.".to_owned())?;
    let _reader = osmpbf::indexed::IndexedReader::from_path(&path).map_err(|error| {
        format!(
            "Failed to initialize Rust OSM PBF reader for {}: {error}",
            path.display()
        )
    })?;
    Ok(())
}

pub fn load_roads_for_bounds(
    selected_root: Option<&Path>,
    bounds: GeoBounds,
    tile_zoom: u8,
    layer_kind: RoadLayerKind,
) -> Vec<RoadPolyline> {
    let Some(db_path) = runtime_db_path(selected_root) else {
        return Vec::new();
    };
    if !db_path.exists() {
        return Vec::new();
    }

    let Ok(connection) = open_runtime_db(&db_path) else {
        return Vec::new();
    };
    let (min_x, min_y) = lat_lon_to_tile(bounds.max_lat, bounds.min_lon, tile_zoom);
    let (max_x, max_y) = lat_lon_to_tile(bounds.min_lat, bounds.max_lon, tile_zoom);
    let Ok(mut statement) = connection.prepare(
        "SELECT way_id, class, name, geom_wkb
         FROM road_tiles
         WHERE zoom = ?1
           AND tile_x BETWEEN ?2 AND ?3
           AND tile_y BETWEEN ?4 AND ?5",
    ) else {
        return Vec::new();
    };

    let rows = match statement.query_map(
        params![
            i64::from(tile_zoom),
            i64::from(min_x.min(max_x)),
            i64::from(min_x.max(max_x)),
            i64::from(min_y.min(max_y)),
            i64::from(min_y.max(max_y)),
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        },
    ) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };

    let mut seen_way_ids = HashSet::new();
    let mut roads = Vec::new();
    for row in rows.filter_map(Result::ok) {
        let (way_id, road_class, name, geom_wkb) = row;
        if !road_class_matches(&road_class, layer_kind) || !seen_way_ids.insert(way_id) {
            continue;
        }
        let Some(points) = decode_linestring_wkb(&geom_wkb) else {
            continue;
        };
        let road_bounds = polyline_bounds(&points);
        if !bounds_intersect(road_bounds, bounds) {
            continue;
        }
        roads.push(RoadPolyline {
            way_id,
            road_class,
            name: if name.is_empty() { None } else { Some(name) },
            points,
        });
    }

    roads
}

pub fn supports_locations_on_ways(selected_root: Option<&Path>) -> Result<bool, String> {
    let path = find_planet_pbf(selected_root)
        .ok_or_else(|| "No planet-latest.osm.pbf source found for validation.".to_owned())?;
    let mut reader = BlobReader::from_path(&path).map_err(|error| error.to_string())?;
    let Some(blob) = reader.next() else {
        return Err(format!("OSM planet file {} is empty.", path.display()));
    };
    let blob = blob.map_err(|error| error.to_string())?;
    let header = match blob.decode().map_err(|error| error.to_string())? {
        BlobDecode::OsmHeader(header) => header,
        _ => {
            return Err(format!(
                "OSM planet file {} did not begin with a valid header block.",
                path.display()
            ));
        }
    };

    Ok(header
        .optional_features()
        .iter()
        .any(|feature| feature == "LocationsOnWays"))
}

fn find_planet_from(root: &Path) -> Option<PathBuf> {
    if root.is_file()
        && root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == PLANET_PBF_NAME)
    {
        return Some(root.to_path_buf());
    }

    if let Some(candidate) = [
        root.join(PLANET_PBF_NAME),
        root.join("Data").join(PLANET_PBF_NAME),
    ]
    .into_iter()
    .find(|candidate| candidate.exists())
    {
        return Some(candidate);
    }

    root.ancestors().find_map(|ancestor| {
        [
            ancestor.join(PLANET_PBF_NAME),
            ancestor.join("Data").join(PLANET_PBF_NAME),
            ancestor.join("data").join(PLANET_PBF_NAME),
        ]
        .into_iter()
        .find(|candidate| candidate.exists())
    })
}

fn runtime_db_path(selected_root: Option<&Path>) -> Option<PathBuf> {
    let derived_root = crate::terrain_assets::find_derived_root(selected_root)?;
    Some(derived_root.join("osm").join(RUNTIME_DB_NAME))
}

fn open_runtime_db(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(connection)
}

fn worker() -> &'static Mutex<Option<ActiveWorker>> {
    static WORKER: OnceLock<Mutex<Option<ActiveWorker>>> = OnceLock::new();
    WORKER.get_or_init(|| Mutex::new(None))
}

fn ensure_runtime_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS osm_sources (
            path TEXT PRIMARY KEY,
            source_kind TEXT NOT NULL,
            file_size_bytes INTEGER NOT NULL,
            modified_at_unix INTEGER NOT NULL,
            detected_at_unix INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS osm_ingest_jobs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            feature_kind TEXT NOT NULL,
            state TEXT NOT NULL,
            source_path TEXT NOT NULL,
            min_lat REAL NOT NULL,
            max_lat REAL NOT NULL,
            min_lon REAL NOT NULL,
            max_lon REAL NOT NULL,
            priority INTEGER NOT NULL DEFAULT 0,
            requested_at_unix INTEGER NOT NULL,
            updated_at_unix INTEGER NOT NULL,
            note TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_osm_ingest_jobs_state
            ON osm_ingest_jobs(state, feature_kind);
        CREATE TABLE IF NOT EXISTS road_tile_manifest (
            zoom INTEGER NOT NULL,
            tile_x INTEGER NOT NULL,
            tile_y INTEGER NOT NULL,
            feature_count INTEGER NOT NULL,
            built_at_unix INTEGER NOT NULL,
            PRIMARY KEY (zoom, tile_x, tile_y)
        );
        CREATE TABLE IF NOT EXISTS road_tiles (
            zoom INTEGER NOT NULL,
            tile_x INTEGER NOT NULL,
            tile_y INTEGER NOT NULL,
            way_id INTEGER NOT NULL,
            class TEXT NOT NULL,
            name TEXT,
            geom_wkb BLOB NOT NULL,
            min_lat REAL NOT NULL,
            max_lat REAL NOT NULL,
            min_lon REAL NOT NULL,
            max_lon REAL NOT NULL,
            PRIMARY KEY (zoom, tile_x, tile_y, way_id)
        );
        CREATE INDEX IF NOT EXISTS idx_road_tiles_lookup
            ON road_tiles(zoom, tile_x, tile_y);
        CREATE TABLE IF NOT EXISTS building_tile_manifest (
            zoom INTEGER NOT NULL,
            tile_x INTEGER NOT NULL,
            tile_y INTEGER NOT NULL,
            feature_count INTEGER NOT NULL,
            built_at_unix INTEGER NOT NULL,
            PRIMARY KEY (zoom, tile_x, tile_y)
        );
        CREATE TABLE IF NOT EXISTS building_tiles (
            zoom INTEGER NOT NULL,
            tile_x INTEGER NOT NULL,
            tile_y INTEGER NOT NULL,
            object_id INTEGER NOT NULL,
            kind TEXT NOT NULL,
            name TEXT,
            geom_wkb BLOB NOT NULL,
            min_lat REAL NOT NULL,
            max_lat REAL NOT NULL,
            min_lon REAL NOT NULL,
            max_lon REAL NOT NULL,
            PRIMARY KEY (zoom, tile_x, tile_y, object_id)
        );
        CREATE INDEX IF NOT EXISTS idx_building_tiles_lookup
            ON building_tiles(zoom, tile_x, tile_y);
        CREATE TABLE IF NOT EXISTS water_tiles (
            zoom INTEGER NOT NULL,
            tile_x INTEGER NOT NULL,
            tile_y INTEGER NOT NULL,
            way_id INTEGER NOT NULL,
            class TEXT NOT NULL,
            name TEXT,
            is_area INTEGER NOT NULL DEFAULT 0,
            geom_wkb BLOB NOT NULL,
            min_lat REAL NOT NULL,
            max_lat REAL NOT NULL,
            min_lon REAL NOT NULL,
            max_lon REAL NOT NULL,
            PRIMARY KEY (zoom, tile_x, tile_y, way_id)
        );
        CREATE INDEX IF NOT EXISTS idx_water_tiles_lookup
            ON water_tiles(zoom, tile_x, tile_y);",
    )?;
    let _ = connection.execute(
        "ALTER TABLE osm_ingest_jobs ADD COLUMN priority INTEGER NOT NULL DEFAULT 0",
        [],
    );
    Ok(())
}

fn has_job_note(selected_root: Option<&Path>, note: &str) -> Result<bool, String> {
    let Some(db_path) = runtime_db_path(selected_root) else {
        return Ok(false);
    };
    if !db_path.exists() {
        return Ok(false);
    }
    let connection = open_runtime_db(&db_path).map_err(|error| error.to_string())?;
    let count: usize = connection
        .query_row(
            "SELECT COUNT(*) FROM osm_ingest_jobs WHERE note = ?1 AND state IN ('queued', 'running', 'completed')",
            params![note],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(count > 0)
}

fn fetch_next_job(connection: &Connection) -> rusqlite::Result<Option<OsmJob>> {
    let mut statement = connection.prepare(
        "SELECT id, feature_kind, source_path, min_lat, max_lat, min_lon, max_lon, note
         FROM osm_ingest_jobs
         WHERE state = 'queued'
         ORDER BY priority DESC, requested_at_unix ASC
         LIMIT 1",
    )?;

    let job = statement
        .query_row([], |row| {
            let feature_kind = match row.get::<_, String>(1)?.as_str() {
                "roads"     => OsmFeatureKind::Roads,
                "buildings" => OsmFeatureKind::Buildings,
                "water"     => OsmFeatureKind::Water,
                _           => OsmFeatureKind::Roads,
            };
            Ok(OsmJob {
                id: row.get(0)?,
                feature_kind,
                source_path: PathBuf::from(row.get::<_, String>(2)?),
                bounds: GeoBounds {
                    min_lat: row.get(3)?,
                    max_lat: row.get(4)?,
                    min_lon: row.get(5)?,
                    max_lon: row.get(6)?,
                },
                note: row.get(7)?,
            })
        })
        .optional()?;

    if let Some(job) = job {
        connection.execute(
            "UPDATE osm_ingest_jobs
             SET state = 'running', updated_at_unix = ?2
             WHERE id = ?1",
            params![job.id, unix_timestamp()],
        )?;
        Ok(Some(job))
    } else {
        Ok(None)
    }
}

fn mark_job_completed(db_path: &Path, job_id: i64, note: &str) -> Result<(), String> {
    let connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE osm_ingest_jobs
             SET state = 'completed', updated_at_unix = ?2, note = ?3
             WHERE id = ?1",
            params![job_id, unix_timestamp(), note],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn mark_job_failed(db_path: &Path, job_id: i64, error_text: &str) -> Result<(), String> {
    let connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE osm_ingest_jobs
             SET state = 'failed', updated_at_unix = ?2, note = ?3
             WHERE id = ?1",
            params![job_id, unix_timestamp(), error_text],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn update_job_note(db_path: &Path, job_id: i64, note: &str) -> Result<(), String> {
    let connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE osm_ingest_jobs
             SET updated_at_unix = ?2, note = ?3
             WHERE id = ?1",
            params![job_id, unix_timestamp(), note],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn import_planet_roads(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    if job.note.starts_with(FOCUS_ROADS_NOTE_PREFIX) {
        // Overpass: either explicitly requested or no planet file.
        if job.source_path == std::path::Path::new(OVERPASS_SOURCE) {
            return import_focus_roads_via_overpass(db_path, job);
        }

        // Planet file available.  Prefer Overpass when the user has opted in,
        // otherwise try osmium extract → stream scan → ogr2ogr → Overpass.
        if settings_store::prefer_overpass() {
            return import_focus_roads_via_overpass(db_path, job);
        }

        return import_focus_roads_via_osmium(db_path, job)
            .or_else(|osmium_err| {
                let _ = update_job_note(
                    db_path, job.id,
                    &format!("osmium unavailable ({osmium_err}); trying stream scan…"),
                );
                import_focus_roads_via_stream_scan(db_path, job)
            })
            .or_else(|scan_err| {
                let _ = update_job_note(
                    db_path, job.id,
                    &format!("Stream scan failed ({scan_err}); falling back to Overpass…"),
                );
                import_focus_roads_via_overpass(db_path, job)
            });
    }

    if !supports_locations_on_ways_for_path(&job.source_path)? {
        return Err(
            "Planet source does not advertise LocationsOnWays; pure-Rust global road bootstrap is not available on this file yet.".to_owned(),
        );
    }

    let connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    connection
        .execute_batch("BEGIN IMMEDIATE; DELETE FROM road_tiles; DELETE FROM road_tile_manifest;")
        .map_err(|error| error.to_string())?;

    let mut writer = RoadTileWriter::new(connection);
    let reader = ElementReader::from_path(&job.source_path).map_err(|error| {
        format!(
            "Failed to open OSM planet source {}: {error}",
            job.source_path.display()
        )
    })?;

    let mut processed_ways = 0usize;
    let mut import_error: Option<String> = None;
    let mut seen_way_ids = HashSet::new();
    reader
        .for_each(|element| {
            if import_error.is_some() {
                return;
            }
            let Element::Way(way) = element else {
                return;
            };

            let mut highway_class = None;
            let mut road_name = None;
            for (key, value) in way.tags() {
                if key == "highway" {
                    highway_class = canonical_road_class(value);
                } else if key == "name" && road_name.is_none() {
                    road_name = Some(value.to_owned());
                }
            }

            let Some(road_class) = highway_class else {
                return;
            };
            if !seen_way_ids.insert(way.id()) {
                return;
            }

            let points: Vec<_> = way
                .node_locations()
                .map(|location| GeoPoint {
                    lat: location.lat() as f32,
                    lon: location.lon() as f32,
                })
                .collect();

            if points.len() < 2 {
                return;
            }
            let bounds = polyline_bounds(&points);
            if !bounds_intersect(bounds, job.bounds) {
                return;
            }

            processed_ways += 1;
            if let Err(error) =
                writer.insert_road(way.id(), road_class, road_name.as_deref(), &points)
            {
                import_error = Some(error);
                return;
            }

            if processed_ways % PROGRESS_FLUSH_INTERVAL == 0 {
                let _ = writer.flush_progress();
                let _ = update_job_note(
                    db_path,
                    job.id,
                    &format!(
                        "Scanned {} road ways · wrote {} tile features",
                        processed_ways, writer.inserted_features
                    ),
                );
            }
        })
        .map_err(|error| error.to_string())?;

    if let Some(error) = import_error {
        let _ = writer.rollback();
        return Err(error);
    }

    let inserted_features = writer.inserted_features;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(format!(
        "Imported {} road ways into {} tile features across {} zoom levels",
        processed_ways,
        inserted_features,
        ROAD_TILE_ZOOMS.len()
    ))
}

// ── Overpass API import ───────────────────────────────────────────────────

/// Fetch roads for `job.bounds` from the Overpass API and insert them into
/// the local SQLite cache using the same `RoadTileWriter` machinery as the
/// planet-file path.  No local OSM data is required.
/// Extract a 1°×1° cell from the planet file with `osmium extract`, cache
/// the resulting small .osm.pbf, then hand it to the stream scanner.
/// Subsequent visits to the same cell skip the osmium step entirely.
fn import_focus_roads_via_osmium(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    // Verify osmium is reachable before we commit to this path.
    let osmium = settings_store::resolve_osmium();
    let probe = Command::new(&osmium).arg("--version").output();
    if probe.is_err() {
        return Err(format!("osmium not found at {}", osmium.display()));
    }

    // Quantise the focus centre to a 1°×1° cell so nearby queries share
    // a single extract and we never re-extract the same cell.
    let lat_cell = job.bounds.min_lat.floor() as i32;
    let lon_cell = job.bounds.min_lon.floor() as i32;

    let extract_dir = db_path
        .parent()
        .ok_or("OSM runtime DB has no parent directory")?
        .join("osm_extracts");
    fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;

    let extract_path = extract_dir
        .join(format!("cell_{:+04}_{:+05}.osm.pbf", lat_cell, lon_cell));

    if !extract_path.exists() {
        // osmium bbox order: lon_min,lat_min,lon_max,lat_max
        let bbox = format!("{},{},{},{}", lon_cell, lat_cell, lon_cell + 1, lat_cell + 1);
        update_job_note(
            db_path,
            job.id,
            &format!(
                "Extracting cell ({lat_cell}°, {lon_cell}°) from planet file with osmium \
                 (one-time per cell, ~2-5 min)…"
            ),
        )?;

        let status = Command::new(&osmium)
            .arg("extract")
            .arg("-b").arg(&bbox)
            .arg(&job.source_path)
            .arg("-o").arg(&extract_path)
            .arg("--overwrite")
            .status()
            .map_err(|e| format!("Failed to launch osmium: {e}"))?;

        if !status.success() {
            let _ = fs::remove_file(&extract_path); // clean up partial output
            return Err(format!("osmium extract exited with status {status}"));
        }
    } else {
        update_job_note(
            db_path,
            job.id,
            &format!("Using cached osmium extract for cell ({lat_cell}°, {lon_cell}°)"),
        )?;
    }

    // Run the fast stream scan on the small regional file.
    let mut scan_job = job.clone();
    scan_job.source_path = extract_path;
    import_focus_roads_via_stream_scan(db_path, &scan_job)
}

fn import_focus_roads_via_overpass(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    update_job_note(db_path, job.id, "Querying Overpass API for road geometry…")?;

    let b = job.bounds;
    // `out geom` returns node coordinates inline — no separate node lookup needed.
    let query = format!(
        "[out:json][timeout:30];\
         way[\"highway\"~\"^(motorway|motorway_link|trunk|trunk_link|\
         primary|primary_link|secondary|secondary_link|\
         tertiary|tertiary_link|residential|living_street|unclassified|service)$\"]\
         ({min_lat},{min_lon},{max_lat},{max_lon});\
         out geom;",
        min_lat = b.min_lat,
        min_lon = b.min_lon,
        max_lat = b.max_lat,
        max_lon = b.max_lon,
    );

    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(40))
        .user_agent("1kEE/0.1 (tactical globe; overpass road fetch)")
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?
        .post(OVERPASS_ENDPOINT)
        .body(query)
        .send()
        .map_err(|e| format!("Overpass request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("Overpass returned HTTP {}", response.status()));
    }

    let text = response.text().map_err(|e| format!("Reading Overpass response: {e}"))?;
    update_job_note(db_path, job.id, "Parsing road geometry from Overpass response…")?;

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Overpass JSON parse error: {e}"))?;

    let elements = json
        .get("elements")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Overpass response missing 'elements' array".to_owned())?;

    let connection = open_runtime_db(db_path).map_err(|e| e.to_string())?;
    connection
        .execute_batch("BEGIN IMMEDIATE;")
        .map_err(|e| e.to_string())?;
    let mut writer = RoadTileWriter::new(connection);

    let mut imported = 0usize;
    let mut skipped = 0usize;

    for element in elements {
        // Only process ways (Overpass can also return nodes/relations).
        if element.get("type").and_then(|v| v.as_str()) != Some("way") {
            continue;
        }

        let way_id = element
            .get("id")
            .and_then(|v| v.as_i64())
            .unwrap_or_default();

        let tags = element.get("tags");
        let highway_val = tags
            .and_then(|t| t.get("highway"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(road_class) = canonical_road_class(highway_val) else {
            skipped += 1;
            continue;
        };

        let road_name = tags
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str());

        // `out geom` puts node coordinates directly in a "geometry" array.
        let geometry = element.get("geometry").and_then(|v| v.as_array());
        let Some(geometry) = geometry else {
            skipped += 1;
            continue;
        };

        let points: Vec<GeoPoint> = geometry
            .iter()
            .filter_map(|node| {
                let lat = node.get("lat")?.as_f64()? as f32;
                let lon = node.get("lon")?.as_f64()? as f32;
                Some(GeoPoint { lat, lon })
            })
            .collect();

        if points.len() < 2 {
            skipped += 1;
            continue;
        }

        writer
            .insert_road(way_id, road_class, road_name, &points)
            .map_err(|e| format!("DB insert error: {e}"))?;
        imported += 1;

        if imported % PROGRESS_FLUSH_INTERVAL == 0 {
            let _ = writer.flush_progress();
            let _ = update_job_note(
                db_path,
                job.id,
                &format!("Importing Overpass roads… {imported} written"),
            );
        }
    }

    writer.finish().map_err(|e| e.to_string())?;
    crate::app::request_repaint();

    Ok(format!(
        "Overpass import complete: {imported} roads written, {skipped} skipped"
    ))
}

// ── Planet-file stream scan ───────────────────────────────────────────────

fn import_focus_roads_via_stream_scan(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    update_job_note(
        db_path,
        job.id,
        "Scanning focused road geometry from the planet source...",
    )?;

    let expanded_bounds = expand_bounds(job.bounds, FOCUS_NODE_MARGIN_DEGREES);
    let connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    connection
        .execute_batch("BEGIN IMMEDIATE;")
        .map_err(|error| error.to_string())?;
    let mut writer = RoadTileWriter::new(connection);

    let reader = ElementReader::from_path(&job.source_path).map_err(|error| {
        format!(
            "Failed to open OSM planet source {}: {error}",
            job.source_path.display()
        )
    })?;

    let mut candidate_nodes: HashMap<i64, GeoPoint> = HashMap::new();
    let mut seen_way_ids = HashSet::new();
    let mut scanned_nodes = 0usize;
    let mut scanned_ways = 0usize;
    let mut imported_roads = 0usize;
    let mut import_error: Option<String> = None;

    reader
        .for_each(|element| {
            if import_error.is_some() {
                return;
            }

            match element {
                Element::Node(node) => {
                    scanned_nodes += 1;
                    let point = GeoPoint {
                        lat: node.lat() as f32,
                        lon: node.lon() as f32,
                    };
                    if point_in_bounds(point, expanded_bounds) {
                        candidate_nodes.insert(node.id(), point);
                    }
                }
                Element::DenseNode(node) => {
                    scanned_nodes += 1;
                    let point = GeoPoint {
                        lat: node.lat() as f32,
                        lon: node.lon() as f32,
                    };
                    if point_in_bounds(point, expanded_bounds) {
                        candidate_nodes.insert(node.id(), point);
                    }
                }
                Element::Way(way) => {
                    scanned_ways += 1;

                    let mut highway_class = None;
                    let mut road_name = None;
                    for (key, value) in way.tags() {
                        if key == "highway" {
                            highway_class = canonical_road_class(value);
                        } else if key == "name" && road_name.is_none() {
                            road_name = Some(value.to_owned());
                        }
                    }
                    let Some(road_class) = highway_class else {
                        return;
                    };
                    if !seen_way_ids.insert(way.id()) {
                        return;
                    }

                    let points: Vec<_> = way
                        .refs()
                        .filter_map(|node_id| candidate_nodes.get(&node_id).copied())
                        .collect();
                    if points.len() < 2 {
                        return;
                    }

                    let bounds = polyline_bounds(&points);
                    if !bounds_intersect(bounds, job.bounds) {
                        return;
                    }

                    imported_roads += 1;
                    if let Err(error) =
                        writer.insert_road(way.id(), road_class, road_name.as_deref(), &points)
                    {
                        import_error = Some(error);
                        return;
                    }

                    if imported_roads % PROGRESS_FLUSH_INTERVAL == 0 {
                        let _ = writer.flush_progress();
                    }
                }
                Element::Relation(_) => {}
            }

            let processed = scanned_nodes.saturating_add(scanned_ways);
            if processed > 0 && processed % FOCUS_SCAN_PROGRESS_INTERVAL == 0 {
                let _ = update_job_note(
                    db_path,
                    job.id,
                    &format!(
                        "Scanned {} nodes, {} ways · kept {} candidate nodes · imported {} roads",
                        scanned_nodes,
                        scanned_ways,
                        candidate_nodes.len(),
                        imported_roads
                    ),
                );
            }
        })
        .map_err(|error| error.to_string())?;

    if let Some(error) = import_error {
        let _ = writer.rollback();
        return Err(error);
    }

    let inserted_features = writer.inserted_features;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(format!(
        "Imported {} focused roads from {} kept nodes into {} tile features",
        imported_roads,
        candidate_nodes.len(),
        inserted_features
    ))
}

fn import_focus_roads_via_ogr2ogr(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    let output_dir = db_path
        .parent()
        .ok_or_else(|| "OSM runtime DB is missing a parent directory.".to_owned())?
        .join("tmp");
    fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;
    let output_path = output_dir.join(format!("osm_focus_job_{}.geojson", job.id));
    if output_path.exists() {
        let _ = fs::remove_file(&output_path);
    }

    update_job_note(
        db_path,
        job.id,
        "Extracting focused road geometry from the planet source...",
    )?;

    let status = Command::new(settings_store::resolve_gdal_tool("ogr2ogr"))
        .arg("-f")
        .arg("GeoJSON")
        .arg(&output_path)
        .arg(&job.source_path)
        .arg("lines")
        .arg("-spat")
        .arg(job.bounds.min_lon.to_string())
        .arg(job.bounds.min_lat.to_string())
        .arg(job.bounds.max_lon.to_string())
        .arg(job.bounds.max_lat.to_string())
        .arg("-where")
        .arg("highway IS NOT NULL")
        .status()
        .map_err(|error| format!("Failed to launch ogr2ogr focused-road import: {error}"))?;

    if !status.success() {
        return Err(format!(
            "ogr2ogr focused-road import failed with status {}",
            status
        ));
    }

    update_job_note(
        db_path,
        job.id,
        "Parsing focused road geometry into the shared tile store...",
    )?;

    let geojson_text = fs::read_to_string(&output_path)
        .map_err(|error| format!("Failed to read {}: {error}", output_path.display()))?;
    let geojson: Value =
        serde_json::from_str(&geojson_text).map_err(|error| format!("Invalid GeoJSON: {error}"))?;

    let connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    connection
        .execute_batch("BEGIN IMMEDIATE;")
        .map_err(|error| error.to_string())?;

    let mut writer = RoadTileWriter::new(connection);
    let mut synthetic_way_id = -1_i64;
    let mut imported_roads = 0usize;
    let mut import_error: Option<String> = None;

    if let Some(features) = geojson.get("features").and_then(Value::as_array) {
        for feature in features {
            if import_error.is_some() {
                break;
            }

            let geometry = feature.get("geometry").unwrap_or(&Value::Null);
            let properties = feature.get("properties").unwrap_or(&Value::Null);
            let highway = properties
                .get("highway")
                .and_then(Value::as_str)
                .and_then(canonical_road_class);
            let Some(road_class) = highway else {
                continue;
            };
            let road_name = properties.get("name").and_then(Value::as_str);
            let way_id = properties
                .get("osm_id")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    let next = synthetic_way_id;
                    synthetic_way_id -= 1;
                    next
                });

            match geometry.get("type").and_then(Value::as_str) {
                Some("LineString") => {
                    if let Some(points) = parse_geojson_linestring(geometry) {
                        if points.len() >= 2 {
                            imported_roads += 1;
                            if let Err(error) =
                                writer.insert_road(way_id, road_class, road_name, &points)
                            {
                                import_error = Some(error);
                            }
                        }
                    }
                }
                Some("MultiLineString") => {
                    if let Some(lines) = parse_geojson_multilinestring(geometry) {
                        for points in lines {
                            if points.len() < 2 {
                                continue;
                            }
                            imported_roads += 1;
                            if let Err(error) =
                                writer.insert_road(way_id, road_class, road_name, &points)
                            {
                                import_error = Some(error);
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let _ = fs::remove_file(&output_path);

    if let Some(error) = import_error {
        let _ = writer.rollback();
        return Err(error);
    }

    let inserted_features = writer.inserted_features;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(format!(
        "Imported {} focused road geometries into {} tile features",
        imported_roads, inserted_features
    ))
}

fn supports_locations_on_ways_for_path(path: &Path) -> Result<bool, String> {
    let mut reader = BlobReader::from_path(path).map_err(|error| error.to_string())?;
    let Some(blob) = reader.next() else {
        return Err(format!("OSM planet file {} is empty.", path.display()));
    };
    let blob = blob.map_err(|error| error.to_string())?;
    let header = match blob.decode().map_err(|error| error.to_string())? {
        BlobDecode::OsmHeader(header) => header,
        _ => {
            return Err(format!(
                "OSM planet file {} did not begin with a valid header block.",
                path.display()
            ));
        }
    };

    Ok(header
        .optional_features()
        .iter()
        .any(|feature| feature == "LocationsOnWays"))
}

fn recover_orphaned_running_jobs(db_path: &Path) -> Result<(), String> {
    let connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE osm_ingest_jobs
             SET state = 'failed',
                 updated_at_unix = ?1,
                 note = 'Recovered orphaned running job; requeue required'
             WHERE state = 'running'",
            params![unix_timestamp()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

struct RoadTileWriter {
    connection: Connection,
    manifest_counts: HashMap<(u8, u32, u32), usize>,
    inserted_features: usize,
}

impl RoadTileWriter {
    fn new(connection: Connection) -> Self {
        Self {
            connection,
            manifest_counts: HashMap::new(),
            inserted_features: 0,
        }
    }

    fn insert_road(
        &mut self,
        way_id: i64,
        road_class: &'static str,
        road_name: Option<&str>,
        points: &[GeoPoint],
    ) -> Result<(), String> {
        let bounds = polyline_bounds(points);
        let wkb = encode_linestring_wkb(points);

        for &zoom in ROAD_TILE_ZOOMS {
            let (min_x, min_y) = lat_lon_to_tile(bounds.max_lat, bounds.min_lon, zoom);
            let (max_x, max_y) = lat_lon_to_tile(bounds.min_lat, bounds.max_lon, zoom);
            for tile_x in min_x.min(max_x)..=min_x.max(max_x) {
                for tile_y in min_y.min(max_y)..=min_y.max(max_y) {
                    self.connection
                        .execute(
                            "INSERT OR REPLACE INTO road_tiles (
                                zoom, tile_x, tile_y, way_id, class, name, geom_wkb,
                                min_lat, max_lat, min_lon, max_lon
                             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                            params![
                                i64::from(zoom),
                                i64::from(tile_x),
                                i64::from(tile_y),
                                way_id,
                                road_class,
                                road_name.unwrap_or(""),
                                &wkb,
                                bounds.min_lat,
                                bounds.max_lat,
                                bounds.min_lon,
                                bounds.max_lon,
                            ],
                        )
                        .map_err(|error| error.to_string())?;
                    *self
                        .manifest_counts
                        .entry((zoom, tile_x, tile_y))
                        .or_insert(0) += 1;
                    self.inserted_features += 1;
                }
            }
        }

        Ok(())
    }

    fn flush_progress(&self) -> Result<(), String> {
        self.connection
            .execute_batch("COMMIT; BEGIN IMMEDIATE;")
            .map_err(|error| error.to_string())
    }

    fn finish(mut self) -> rusqlite::Result<()> {
        let built_at = unix_timestamp();
        for ((zoom, tile_x, tile_y), feature_count) in self.manifest_counts.drain() {
            let live_count: i64 = self.connection.query_row(
                "SELECT COUNT(*) FROM road_tiles
                 WHERE zoom = ?1 AND tile_x = ?2 AND tile_y = ?3",
                params![i64::from(zoom), i64::from(tile_x), i64::from(tile_y)],
                |row| row.get(0),
            )?;
            self.connection.execute(
                "INSERT OR REPLACE INTO road_tile_manifest (
                    zoom, tile_x, tile_y, feature_count, built_at_unix
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    i64::from(zoom),
                    i64::from(tile_x),
                    i64::from(tile_y),
                    live_count.max(feature_count as i64),
                    built_at,
                ],
            )?;
        }

        self.connection.execute_batch("COMMIT;")
    }

    fn rollback(&self) -> Result<(), String> {
        self.connection
            .execute_batch("ROLLBACK;")
            .map_err(|error| error.to_string())
    }
}

// ── Water feature infrastructure ──────────────────────────────────────────

const FOCUS_WATER_NOTE_PREFIX: &str = "focus_water";

/// Load water features for a viewport tile range from the runtime SQLite DB.
pub fn load_water_for_bounds(
    selected_root: Option<&Path>,
    bounds: GeoBounds,
    tile_zoom: u8,
) -> Vec<WaterPolyline> {
    let Some(db_path) = runtime_db_path(selected_root) else { return Vec::new() };
    if !db_path.exists() { return Vec::new() }
    let Ok(conn) = open_runtime_db(&db_path) else { return Vec::new() };

    let (x0, y0) = lat_lon_to_tile(bounds.max_lat, bounds.min_lon, tile_zoom);
    let (x1, y1) = lat_lon_to_tile(bounds.min_lat, bounds.max_lon, tile_zoom);
    let Ok(mut stmt) = conn.prepare(
        "SELECT way_id, class, name, is_area, geom_wkb
         FROM water_tiles
         WHERE zoom=?1 AND tile_x BETWEEN ?2 AND ?3 AND tile_y BETWEEN ?4 AND ?5",
    ) else { return Vec::new() };

    let rows = match stmt.query_map(
        params![
            i64::from(tile_zoom),
            i64::from(x0.min(x1)), i64::from(x0.max(x1)),
            i64::from(y0.min(y1)), i64::from(y0.max(y1)),
        ],
        |row| Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Vec<u8>>(4)?,
        )),
    ) { Ok(r) => r, Err(_) => return Vec::new() };

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for row in rows.filter_map(Result::ok) {
        let (way_id, water_class, name, is_area, wkb) = row;
        if !seen.insert(way_id) { continue; }
        let Some(points) = decode_linestring_wkb(&wkb) else { continue };
        let poly_bounds = polyline_bounds(&points);
        if !bounds_intersect(poly_bounds, bounds) { continue; }
        out.push(WaterPolyline {
            way_id,
            water_class,
            name: if name.is_empty() { None } else { Some(name) },
            points,
            is_area: is_area != 0,
        });
    }
    out
}

/// Queue a focused water feature import for `focus` ± `radius_miles`.
pub fn queue_focus_water_import(
    selected_root: Option<&Path>,
    focus: GeoPoint,
    radius_miles: f32,
) -> Result<bool, String> {
    let radius_miles = radius_miles.clamp(5.0, 150.0);
    let bounds = focus_bounds(focus, radius_miles);
    let radius_bucket = ((radius_miles / 5.0).ceil() as u32) * 5;
    let note = format!(
        "{FOCUS_WATER_NOTE_PREFIX}_{:.3}_{:.3}_r{}",
        focus.lat, focus.lon, radius_bucket
    );

    if known_notes().lock().map(|g| g.contains(&note)).unwrap_or(false) {
        return Ok(false);
    }

    let source_path = if find_planet_pbf(selected_root).is_some()
        && !settings_store::prefer_overpass()
    {
        find_planet_pbf(selected_root).unwrap().to_string_lossy().into_owned()
    } else {
        OVERPASS_SOURCE.to_owned()
    };

    let db_path = ensure_runtime_store(selected_root)?;
    let connection = open_runtime_db(&db_path).map_err(|e| e.to_string())?;

    let existing_count: usize = connection
        .query_row(
            "SELECT COUNT(*) FROM osm_ingest_jobs
             WHERE feature_kind=?1 AND note=?2 AND state IN ('queued','running','completed')",
            params![OsmFeatureKind::Water.as_str(), &note],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if existing_count > 0 {
        if let Ok(mut notes) = known_notes().lock() { notes.insert(note.clone()); }
        return Ok(false);
    }

    let now = unix_timestamp();
    connection
        .execute(
            "INSERT INTO osm_ingest_jobs (
                feature_kind, state, source_path,
                min_lat, max_lat, min_lon, max_lon, priority,
                requested_at_unix, updated_at_unix, note
             ) VALUES (?1,'queued',?2,?3,?4,?5,?6,?7,?8,?8,?9)",
            params![
                OsmFeatureKind::Water.as_str(), source_path,
                bounds.min_lat, bounds.max_lat, bounds.min_lon, bounds.max_lon,
                100_i64, now, &note,
            ],
        )
        .map_err(|e| e.to_string())?;

    if let Ok(mut notes) = known_notes().lock() { notes.insert(note); }
    active_jobs_flag().store(true, Ordering::Relaxed);
    Ok(true)
}

/// Top-level water import dispatcher (called from tick()).
fn import_planet_water(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    if job.source_path == std::path::Path::new(OVERPASS_SOURCE) {
        return import_focus_water_via_overpass(db_path, job);
    }
    if settings_store::prefer_overpass() {
        return import_focus_water_via_overpass(db_path, job);
    }
    import_focus_water_via_osmium(db_path, job)
        .or_else(|e| {
            let _ = update_job_note(db_path, job.id,
                &format!("osmium unavailable ({e}); falling back to Overpass…"));
            import_focus_water_via_overpass(db_path, job)
        })
}

fn import_focus_water_via_osmium(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    let osmium = settings_store::resolve_osmium();
    if std::process::Command::new(&osmium).arg("--version").output().is_err() {
        return Err(format!("osmium not found at {}", osmium.display()));
    }

    let lat_cell = job.bounds.min_lat.floor() as i32;
    let lon_cell = job.bounds.min_lon.floor() as i32;
    let extract_dir = db_path.parent().ok_or("no parent dir")?.join("osm_extracts");
    fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;
    let extract_path = extract_dir.join(format!("cell_{:+04}_{:+05}.osm.pbf", lat_cell, lon_cell));

    if !extract_path.exists() {
        let bbox = format!("{},{},{},{}", lon_cell, lat_cell, lon_cell + 1, lat_cell + 1);
        update_job_note(db_path, job.id,
            &format!("Extracting cell ({lat_cell}°, {lon_cell}°) for water features…"))?;
        let status = Command::new(&osmium)
            .arg("extract").arg("-b").arg(&bbox)
            .arg(&job.source_path).arg("-o").arg(&extract_path).arg("--overwrite")
            .status().map_err(|e| format!("osmium launch failed: {e}"))?;
        if !status.success() {
            let _ = fs::remove_file(&extract_path);
            return Err(format!("osmium extract exited with {status}"));
        }
    }

    let mut scan_job = job.clone();
    scan_job.source_path = extract_path;
    import_focus_water_via_stream_scan(db_path, &scan_job)
}

fn import_focus_water_via_stream_scan(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    update_job_note(db_path, job.id, "Scanning water features from planet source…")?;

    let expanded = expand_bounds(job.bounds, FOCUS_NODE_MARGIN_DEGREES);
    let conn = open_runtime_db(db_path).map_err(|e| e.to_string())?;
    conn.execute_batch("BEGIN IMMEDIATE;").map_err(|e| e.to_string())?;
    let mut writer = WaterTileWriter::new(conn);

    let reader = ElementReader::from_path(&job.source_path)
        .map_err(|e| format!("Failed to open {}: {e}", job.source_path.display()))?;

    let mut candidate_nodes: HashMap<i64, GeoPoint> = HashMap::new();
    let mut seen_way_ids = HashSet::new();
    let mut imported = 0usize;
    let mut import_error: Option<String> = None;

    reader.for_each(|element| {
        if import_error.is_some() { return; }
        match element {
            Element::Node(n) => {
                let pt = GeoPoint { lat: n.lat() as f32, lon: n.lon() as f32 };
                if point_in_bounds(pt, expanded) { candidate_nodes.insert(n.id(), pt); }
            }
            Element::DenseNode(n) => {
                let pt = GeoPoint { lat: n.lat() as f32, lon: n.lon() as f32 };
                if point_in_bounds(pt, expanded) { candidate_nodes.insert(n.id(), pt); }
            }
            Element::Way(way) => {
                let mut water_class: Option<(&'static str, bool)> = None;
                let mut feat_name = None;
                for (k, v) in way.tags() {
                    if water_class.is_none() {
                        water_class = canonical_water_class(k, v);
                    }
                    if k == "name" && feat_name.is_none() {
                        feat_name = Some(v.to_owned());
                    }
                }
                let Some((class, is_area)) = water_class else { return };
                if !seen_way_ids.insert(way.id()) { return; }

                let refs: Vec<i64> = way.refs().collect();
                let closed = refs.first() == refs.last() && refs.len() > 2;
                let is_area = is_area || closed;

                let points: Vec<GeoPoint> = refs.iter()
                    .filter_map(|&id| candidate_nodes.get(&id).copied())
                    .collect();
                if points.len() < 2 { return; }

                let bounds = polyline_bounds(&points);
                if !bounds_intersect(bounds, job.bounds) { return; }

                imported += 1;
                if let Err(e) = writer.insert_water(way.id(), class, feat_name.as_deref(),
                                                     is_area, &points) {
                    import_error = Some(e);
                }
                if imported % PROGRESS_FLUSH_INTERVAL == 0 { let _ = writer.flush_progress(); }
            }
            Element::Relation(_) => {}
        }
    }).map_err(|e| e.to_string())?;

    if let Some(e) = import_error { let _ = writer.rollback(); return Err(e); }
    let total = writer.inserted_features;
    writer.finish_simple().map_err(|e| e.to_string())?;
    Ok(format!("Water stream scan: {imported} features → {total} tile entries"))
}

fn import_focus_water_via_overpass(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    update_job_note(db_path, job.id, "Querying Overpass API for water features…")?;

    let b = job.bounds;
    let query = format!(
        "[out:json][timeout:60];\
         (\
           way[\"waterway\"~\"^(river|stream|canal|drain|creek|ditch)$\"]\
             ({min_lat},{min_lon},{max_lat},{max_lon});\
           way[\"natural\"=\"water\"]\
             ({min_lat},{min_lon},{max_lat},{max_lon});\
           way[\"landuse\"~\"^(reservoir|basin)$\"]\
             ({min_lat},{min_lon},{max_lat},{max_lon});\
         );\
         out geom;",
        min_lat = b.min_lat, min_lon = b.min_lon,
        max_lat = b.max_lat, max_lon = b.max_lon,
    );

    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?
        .post(OVERPASS_ENDPOINT)
        .body(query)
        .send()
        .map_err(|e| format!("Overpass request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("Overpass returned HTTP {}", response.status()));
    }

    let text = response.text().map_err(|e| format!("Failed to read Overpass response: {e}"))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| format!("Invalid JSON: {e}"))?;

    let conn = open_runtime_db(db_path).map_err(|e| e.to_string())?;
    conn.execute_batch("BEGIN IMMEDIATE;").map_err(|e| e.to_string())?;
    let mut writer = WaterTileWriter::new(conn);
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut synthetic_id = -1_i64;

    if let Some(elements) = json.get("elements").and_then(|v| v.as_array()) {
        for element in elements {
            let tags = element.get("tags").and_then(|t| t.as_object());
            let (class, is_area_hint) = tags
                .and_then(|t| {
                    for (k, v) in t {
                        if let Some(r) = canonical_water_class(k, v.as_str().unwrap_or("")) {
                            return Some(r);
                        }
                    }
                    None
                })
                .unwrap_or_else(|| { skipped += 1; ("", false) });
            if class.is_empty() { continue; }

            let name = tags.and_then(|t| t.get("name")).and_then(|v| v.as_str())
                .filter(|s| !s.is_empty()).map(str::to_owned);

            let points: Vec<GeoPoint> = element
                .get("geometry").and_then(|g| g.as_array())
                .map(|pts| pts.iter().filter_map(|p| {
                    Some(GeoPoint {
                        lat: p.get("lat")?.as_f64()? as f32,
                        lon: p.get("lon")?.as_f64()? as f32,
                    })
                }).collect())
                .unwrap_or_default();

            if points.len() < 2 { skipped += 1; continue; }

            let closed = points.first().map(|p| p.lat) == points.last().map(|p| p.lat)
                && points.first().map(|p| p.lon) == points.last().map(|p| p.lon)
                && points.len() > 2;
            let is_area = is_area_hint || closed;

            let way_id = element.get("id").and_then(|v| v.as_i64()).unwrap_or_else(|| {
                synthetic_id -= 1; synthetic_id
            });

            if let Err(e) = writer.insert_water(way_id, class, name.as_deref(), is_area, &points) {
                let _ = writer.rollback();
                return Err(e);
            }
            imported += 1;

            if imported % PROGRESS_FLUSH_INTERVAL == 0 {
                let _ = writer.flush_progress();
                let _ = update_job_note(db_path, job.id,
                    &format!("Overpass water import… {imported} written"));
            }
        }
    }

    writer.finish_simple().map_err(|e| e.to_string())?;
    crate::app::request_repaint();
    Ok(format!("Overpass water import: {imported} features, {skipped} skipped"))
}

// ── WaterTileWriter ────────────────────────────────────────────────────────

struct WaterTileWriter {
    connection: Connection,
    inserted_features: usize,
}

impl WaterTileWriter {
    fn new(connection: Connection) -> Self {
        Self { connection, inserted_features: 0 }
    }

    fn insert_water(&mut self, way_id: i64, class: &str, name: Option<&str>,
                    is_area: bool, points: &[GeoPoint]) -> Result<(), String> {
        let bounds = polyline_bounds(points);
        let wkb = encode_linestring_wkb(points);
        for &zoom in ROAD_TILE_ZOOMS {
            let (min_x, min_y) = lat_lon_to_tile(bounds.max_lat, bounds.min_lon, zoom);
            let (max_x, max_y) = lat_lon_to_tile(bounds.min_lat, bounds.max_lon, zoom);
            for tile_x in min_x.min(max_x)..=min_x.max(max_x) {
                for tile_y in min_y.min(max_y)..=min_y.max(max_y) {
                    self.connection.execute(
                        "INSERT OR REPLACE INTO water_tiles (
                            zoom, tile_x, tile_y, way_id, class, name, is_area,
                            geom_wkb, min_lat, max_lat, min_lon, max_lon
                         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                        params![
                            i64::from(zoom), i64::from(tile_x), i64::from(tile_y),
                            way_id, class, name.unwrap_or(""),
                            if is_area { 1i64 } else { 0i64 },
                            &wkb,
                            bounds.min_lat, bounds.max_lat, bounds.min_lon, bounds.max_lon,
                        ],
                    ).map_err(|e| e.to_string())?;
                    self.inserted_features += 1;
                }
            }
        }
        Ok(())
    }

    fn flush_progress(&self) -> Result<(), String> {
        self.connection.execute_batch("COMMIT; BEGIN IMMEDIATE;").map_err(|e| e.to_string())
    }

    fn finish_simple(mut self) -> rusqlite::Result<()> {
        self.connection.execute_batch("COMMIT;")?;
        Ok(())
    }

    fn rollback(&self) -> Result<(), String> {
        self.connection.execute_batch("ROLLBACK;").map_err(|e| e.to_string())
    }
}

fn canonical_water_class(key: &str, value: &str) -> Option<(&'static str, bool)> {
    match (key, value) {
        ("waterway", "river")               => Some(("river",     false)),
        ("waterway", "stream")
        | ("waterway", "creek")             => Some(("stream",    false)),
        ("waterway", "canal")               => Some(("canal",     false)),
        ("waterway", "drain")
        | ("waterway", "ditch")             => Some(("drain",     false)),
        ("natural",  "water")               => Some(("lake",      true)),
        ("landuse",  "reservoir")
        | ("landuse",  "basin")             => Some(("reservoir", true)),
        _                                   => None,
    }
}

fn canonical_road_class(value: &str) -> Option<&'static str> {
    match value {
        "motorway" | "motorway_link" => Some("motorway"),
        "trunk" | "trunk_link" => Some("trunk"),
        "primary" | "primary_link" => Some("primary"),
        "secondary" | "secondary_link" => Some("secondary"),
        "tertiary" | "tertiary_link" => Some("tertiary"),
        "residential" | "living_street" | "unclassified" => Some("residential"),
        "service" => Some("service"),
        _ => None,
    }
}

fn road_class_matches(road_class: &str, layer_kind: RoadLayerKind) -> bool {
    match layer_kind {
        RoadLayerKind::Major => {
            matches!(road_class, "motorway" | "trunk" | "primary" | "secondary")
        }
        RoadLayerKind::Minor => matches!(road_class, "tertiary" | "residential" | "service"),
    }
}

fn polyline_bounds(points: &[GeoPoint]) -> GeoBounds {
    let mut bounds = GeoBounds {
        min_lat: f32::INFINITY,
        max_lat: f32::NEG_INFINITY,
        min_lon: f32::INFINITY,
        max_lon: f32::NEG_INFINITY,
    };
    for point in points {
        bounds.min_lat = bounds.min_lat.min(point.lat);
        bounds.max_lat = bounds.max_lat.max(point.lat);
        bounds.min_lon = bounds.min_lon.min(point.lon);
        bounds.max_lon = bounds.max_lon.max(point.lon);
    }
    bounds
}

fn bounds_intersect(left: GeoBounds, right: GeoBounds) -> bool {
    left.max_lat >= right.min_lat
        && left.min_lat <= right.max_lat
        && left.max_lon >= right.min_lon
        && left.min_lon <= right.max_lon
}

fn point_in_bounds(point: GeoPoint, bounds: GeoBounds) -> bool {
    point.lat >= bounds.min_lat
        && point.lat <= bounds.max_lat
        && point.lon >= bounds.min_lon
        && point.lon <= bounds.max_lon
}

fn expand_bounds(bounds: GeoBounds, margin_degrees: f32) -> GeoBounds {
    GeoBounds {
        min_lat: (bounds.min_lat - margin_degrees).clamp(-85.0511, 85.0511),
        max_lat: (bounds.max_lat + margin_degrees).clamp(-85.0511, 85.0511),
        min_lon: (bounds.min_lon - margin_degrees).clamp(-180.0, 180.0),
        max_lon: (bounds.max_lon + margin_degrees).clamp(-180.0, 180.0),
    }
}

pub fn lat_lon_to_tile(lat: f32, lon: f32, zoom: u8) -> (u32, u32) {
    let lat = lat.clamp(-85.0511, 85.0511) as f64;
    let lon = lon.clamp(-180.0, 180.0) as f64;
    let zoom_scale = 2_f64.powi(i32::from(zoom));
    let x = ((lon + 180.0) / 360.0 * zoom_scale)
        .floor()
        .clamp(0.0, zoom_scale - 1.0);
    let lat_rad = lat.to_radians();
    let y = ((1.0 - ((lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI)) / 2.0
        * zoom_scale)
        .floor()
        .clamp(0.0, zoom_scale - 1.0);
    (x as u32, y as u32)
}

fn encode_linestring_wkb(points: &[GeoPoint]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(1 + 4 + 4 + points.len() * 16);
    bytes.push(1); // little endian
    bytes.extend_from_slice(&2u32.to_le_bytes()); // LineString
    bytes.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for point in points {
        bytes.extend_from_slice(&(point.lon as f64).to_le_bytes());
        bytes.extend_from_slice(&(point.lat as f64).to_le_bytes());
    }
    bytes
}

fn decode_linestring_wkb(bytes: &[u8]) -> Option<Vec<GeoPoint>> {
    if bytes.len() < 9 {
        return None;
    }
    if *bytes.first()? != 1 {
        return None;
    }
    let geometry_type = u32::from_le_bytes(bytes.get(1..5)?.try_into().ok()?);
    if geometry_type != 2 {
        return None;
    }
    let point_count = u32::from_le_bytes(bytes.get(5..9)?.try_into().ok()?) as usize;
    if bytes.len() < 9 + point_count * 16 {
        return None;
    }

    let mut points = Vec::with_capacity(point_count);
    let mut cursor = 9;
    for _ in 0..point_count {
        let lon = f64::from_le_bytes(bytes.get(cursor..cursor + 8)?.try_into().ok()?);
        let lat = f64::from_le_bytes(bytes.get(cursor + 8..cursor + 16)?.try_into().ok()?);
        cursor += 16;
        points.push(GeoPoint {
            lat: lat as f32,
            lon: lon as f32,
        });
    }
    Some(points)
}

fn parse_geojson_linestring(geometry: &Value) -> Option<Vec<GeoPoint>> {
    let coordinates = geometry.get("coordinates")?.as_array()?;
    let mut points = Vec::with_capacity(coordinates.len());
    for coordinate in coordinates {
        let pair = coordinate.as_array()?;
        let lon = pair.first()?.as_f64()?;
        let lat = pair.get(1)?.as_f64()?;
        points.push(GeoPoint {
            lat: lat as f32,
            lon: lon as f32,
        });
    }
    Some(points)
}

fn parse_geojson_multilinestring(geometry: &Value) -> Option<Vec<Vec<GeoPoint>>> {
    let coordinates = geometry.get("coordinates")?.as_array()?;
    let mut lines = Vec::with_capacity(coordinates.len());
    for line in coordinates {
        let mut points = Vec::new();
        for coordinate in line.as_array()? {
            let pair = coordinate.as_array()?;
            let lon = pair.first()?.as_f64()?;
            let lat = pair.get(1)?.as_f64()?;
            points.push(GeoPoint {
                lat: lat as f32,
                lon: lon as f32,
            });
        }
        lines.push(points);
    }
    Some(lines)
}

fn focus_bounds(focus: GeoPoint, radius_miles: f32) -> GeoBounds {
    let radius_km = radius_miles.max(1.0) * 1.60934;
    let lat_delta = radius_km / 111.32;
    let lon_scale = (focus.lat.to_radians().cos()).abs().max(0.15);
    let lon_delta = radius_km / (111.32 * lon_scale);

    GeoBounds {
        min_lat: (focus.lat - lat_delta).clamp(-85.0511, 85.0511),
        max_lat: (focus.lat + lat_delta).clamp(-85.0511, 85.0511),
        min_lon: (focus.lon - lon_delta).clamp(-180.0, 180.0),
        max_lon: (focus.lon + lon_delta).clamp(-180.0, 180.0),
    }
}

fn register_planet_source(connection: &Connection, path: &Path) -> rusqlite::Result<()> {
    let metadata = fs::metadata(path)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(system_time_to_unix)
        .unwrap_or_default();
    let detected = unix_timestamp();

    connection.execute(
        "INSERT INTO osm_sources (
            path, source_kind, file_size_bytes, modified_at_unix, detected_at_unix
         ) VALUES (?1, 'planet_pbf', ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET
            file_size_bytes = excluded.file_size_bytes,
            modified_at_unix = excluded.modified_at_unix,
            detected_at_unix = excluded.detected_at_unix",
        params![
            path.display().to_string(),
            metadata.len() as i64,
            modified,
            detected,
        ],
    )?;

    Ok(())
}

fn read_runtime_counts(path: &Path) -> rusqlite::Result<(bool, usize, usize, usize, usize)> {
    let connection = open_runtime_db(path)?;
    ensure_runtime_schema(&connection)?;

    let queued_jobs: usize = connection.query_row(
        "SELECT COUNT(*) FROM osm_ingest_jobs WHERE state != 'completed'",
        [],
        |row| row.get(0),
    )?;
    let road_tiles: usize =
        connection.query_row("SELECT COUNT(*) FROM road_tile_manifest", [], |row| {
            row.get(0)
        })?;
    let building_tiles = connection
        .query_row("SELECT COUNT(*) FROM building_tile_manifest", [], |row| {
            row.get(0)
        })
        .optional()?
        .unwrap_or(0);
    let water_tiles = connection
        .query_row("SELECT COUNT(*) FROM water_tiles", [], |row| row.get(0))
        .optional()?
        .unwrap_or(0);

    Ok((true, queued_jobs, road_tiles, building_tiles, water_tiles))
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn system_time_to_unix(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
