use crate::settings_store;
use rusqlite::{Connection, params};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::db::{
    fetch_next_job, mark_job_completed, mark_job_failed, open_runtime_db,
    recover_orphaned_running_jobs, runtime_db_path, update_job_note,
};
use super::inventory::find_planet_pbf;
use super::util::unix_timestamp;
use super::water::import_planet_water;
use super::{ActiveWorker, GeoBounds, OsmFeatureKind, OsmJob, OsmJobSnapshot};

// ---------------------------------------------------------------------------
// In-memory caches — eliminate per-frame SQLite hits on the render thread
// ---------------------------------------------------------------------------

/// In-progress osmium cell extraction counter — (cells_done, cells_total).
fn cell_progress_store() -> &'static Mutex<Option<(u32, u32)>> {
    static P: OnceLock<Mutex<Option<(u32, u32)>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

pub(super) fn set_cell_progress(done: u32, total: u32) {
    if let Ok(mut g) = cell_progress_store().lock() {
        *g = Some((done, total));
    }
}

pub(super) fn clear_cell_progress() {
    if let Ok(mut g) = cell_progress_store().lock() {
        *g = None;
    }
}

/// Returns `(cells_done, cells_total)` while osmium cell extraction is running.
pub fn osmium_cell_progress() -> Option<(u32, u32)> {
    cell_progress_store().lock().ok()?.clone()
}

/// Notes of every job that has ever been queued/completed.
fn known_notes() -> &'static Mutex<HashSet<String>> {
    static CACHE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// True while at least one job is in 'queued' or 'running' state.
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

/// Monotonically increasing counter bumped every time a road import completes.
pub(super) fn road_data_gen() -> &'static AtomicU64 {
    static GEN: OnceLock<AtomicU64> = OnceLock::new();
    GEN.get_or_init(|| AtomicU64::new(0))
}

pub fn road_data_generation() -> u64 {
    road_data_gen().load(Ordering::Relaxed)
}

pub(super) fn water_data_gen() -> &'static AtomicU64 {
    static GEN: OnceLock<AtomicU64> = OnceLock::new();
    GEN.get_or_init(|| AtomicU64::new(0))
}

pub fn water_data_generation() -> u64 {
    water_data_gen().load(Ordering::Relaxed)
}

fn worker() -> &'static Mutex<Option<ActiveWorker>> {
    static WORKER: OnceLock<Mutex<Option<ActiveWorker>>> = OnceLock::new();
    WORKER.get_or_init(|| Mutex::new(None))
}

/// Load known notes and active-job flag from the DB once at startup.
pub(super) fn initialize_caches(db_path: &Path) {
    if caches_initialized().swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = recover_orphaned_running_jobs(db_path);
    let Ok(connection) = open_runtime_db(db_path) else {
        return;
    };

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

pub fn has_active_jobs(_selected_root: Option<&Path>) -> bool {
    active_jobs_flag().load(Ordering::Relaxed)
}

pub fn snapshots(selected_root: Option<&Path>) -> Vec<OsmJobSnapshot> {
    use super::FOCUS_ROADS_NOTE_PREFIX;
    use super::PLANET_ROADS_NOTE;

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

pub fn ensure_runtime_store(selected_root: Option<&Path>) -> Result<PathBuf, String> {
    use super::db::{ensure_runtime_schema, register_planet_source};

    let db_path = runtime_db_path(selected_root).ok_or_else(|| {
        "Unable to resolve Derived/ root for the shared OSM runtime store.".to_owned()
    })?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let connection = open_runtime_db(&db_path).map_err(|error| error.to_string())?;
    ensure_runtime_schema(&connection).map_err(|error| error.to_string())?;

    if let Some(planet_path) = find_planet_pbf(selected_root) {
        register_planet_source(&connection, &planet_path).map_err(|error| error.to_string())?;
    }

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
    use super::PLANET_ROADS_NOTE;

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
    focus: crate::model::GeoPoint,
    radius_miles: f32,
) -> Result<bool, String> {
    use super::util::focus_bounds;
    use super::{FOCUS_ROADS_NOTE_PREFIX, OVERPASS_SOURCE};

    let radius_miles = radius_miles.clamp(5.0, 150.0);
    let bounds = focus_bounds(focus, radius_miles);
    let radius_bucket = ((radius_miles / 5.0).ceil() as u32) * 5;
    let cells = focus_cells_for_bounds(bounds);
    let cell_bounds = focus_cells_bounds(&cells);
    let note = format!(
        "{FOCUS_ROADS_NOTE_PREFIX}_cells_{}_{}_{}_{}_r{}",
        cell_bounds.min_lat.floor() as i32,
        cell_bounds.min_lon.floor() as i32,
        cell_bounds.max_lat.ceil() as i32,
        cell_bounds.max_lon.ceil() as i32,
        radius_bucket
    );

    if known_notes()
        .lock()
        .map(|g| g.contains(&note))
        .unwrap_or(false)
    {
        return Ok(false);
    }

    let source_path =
        if find_planet_pbf(selected_root).is_some() && !settings_store::prefer_overpass() {
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

    if let Ok(mut notes) = known_notes().lock() {
        notes.insert(note);
    }
    active_jobs_flag().store(true, Ordering::Relaxed);

    Ok(true)
}

pub fn queue_focus_water_import(
    selected_root: Option<&Path>,
    focus: crate::model::GeoPoint,
    radius_miles: f32,
) -> Result<bool, String> {
    use super::util::focus_bounds;
    use super::{FOCUS_WATER_NOTE_PREFIX, OVERPASS_SOURCE};

    let radius_miles = radius_miles.clamp(5.0, 150.0);
    let bounds = focus_bounds(focus, radius_miles);
    let radius_bucket = ((radius_miles / 5.0).ceil() as u32) * 5;
    let note = format!(
        "{FOCUS_WATER_NOTE_PREFIX}_{:.3}_{:.3}_r{}",
        focus.lat, focus.lon, radius_bucket
    );

    if known_notes()
        .lock()
        .map(|g| g.contains(&note))
        .unwrap_or(false)
    {
        return Ok(false);
    }

    let source_path =
        if find_planet_pbf(selected_root).is_some() && !settings_store::prefer_overpass() {
            find_planet_pbf(selected_root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
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
             ) VALUES (?1,'queued',?2,?3,?4,?5,?6,?7,?8,?8,?9)",
            params![
                OsmFeatureKind::Water.as_str(),
                source_path,
                bounds.min_lat,
                bounds.max_lat,
                bounds.min_lon,
                bounds.max_lon,
                100_i64,
                now,
                &note,
            ],
        )
        .map_err(|e| e.to_string())?;

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
            let active = guard.take().expect("finished worker present");
            drop(guard);
            let _ = active.handle.join();
            if let Ok(mut note) = current_job_note_store().lock() {
                *note = None;
            }
        } else {
            return;
        }
    } else {
        drop(guard);
        if !active_jobs_flag().load(Ordering::Relaxed) {
            return;
        }
    }

    let Ok(db_path) = ensure_runtime_store(selected_root) else {
        return;
    };

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
        active_jobs_flag().store(false, Ordering::Relaxed);
        if let Ok(mut note) = current_job_note_store().lock() {
            *note = None;
        }
        return;
    };
    drop(connection);

    if let Ok(mut note) = current_job_note_store().lock() {
        *note = Some(job.note.clone());
    }

    let handle = thread::spawn(move || {
        let result = match job.feature_kind {
            OsmFeatureKind::Roads => import_planet_roads_dispatch(&db_path, &job),
            OsmFeatureKind::Water => import_planet_water(&db_path, &job),
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

        crate::app::request_repaint();
    });

    if let Ok(mut guard) = worker.lock() {
        *guard = Some(ActiveWorker { handle });
    }
}

fn import_planet_roads_dispatch(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    use super::roads_global::import_planet_roads;
    use super::roads_osmium::import_focus_roads_via_osmium;
    use super::roads_overpass::import_focus_roads_via_overpass;
    use super::{FOCUS_ROADS_NOTE_PREFIX, OVERPASS_SOURCE};

    if job.note.starts_with(FOCUS_ROADS_NOTE_PREFIX) {
        if job.source_path == std::path::Path::new(OVERPASS_SOURCE) {
            return import_focus_roads_via_overpass(db_path, job);
        }

        if settings_store::prefer_overpass() {
            return import_focus_roads_via_overpass(db_path, job);
        }

        return import_focus_roads_via_osmium(db_path, job)
            .or_else(|osmium_err| {
                let _ = update_job_note(
                    db_path,
                    job.id,
                    &format!("osmium/vector-cache path failed ({osmium_err}); falling back to Overpass…"),
                );
                import_focus_roads_via_overpass(db_path, job)
            });
    }

    import_planet_roads(db_path, job)
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

// ---------------------------------------------------------------------------
// Focus-cell helpers (used by roads_osmium and water)
// ---------------------------------------------------------------------------

pub(super) fn focus_cells_for_bounds(bounds: GeoBounds) -> Vec<(i32, i32)> {
    let min_lat_c = bounds.min_lat.floor() as i32;
    let max_lat_c = bounds.max_lat.floor() as i32;
    let min_lon_c = bounds.min_lon.floor() as i32;
    let max_lon_c = bounds.max_lon.floor() as i32;
    (min_lat_c..=max_lat_c)
        .flat_map(|lat| (min_lon_c..=max_lon_c).map(move |lon| (lat, lon)))
        .collect()
}

pub(super) fn focus_cells_bounds(cells: &[(i32, i32)]) -> GeoBounds {
    let mut min_lat = i32::MAX;
    let mut max_lat = i32::MIN;
    let mut min_lon = i32::MAX;
    let mut max_lon = i32::MIN;
    for &(cell_lat, cell_lon) in cells {
        min_lat = min_lat.min(cell_lat);
        max_lat = max_lat.max(cell_lat);
        min_lon = min_lon.min(cell_lon);
        max_lon = max_lon.max(cell_lon);
    }
    GeoBounds {
        min_lat: (min_lat as f32).clamp(-85.0511, 85.0511),
        max_lat: ((max_lat + 1) as f32).clamp(-85.0511, 85.0511),
        min_lon: (min_lon as f32).clamp(-180.0, 180.0),
        max_lon: ((max_lon + 1) as f32).clamp(-180.0, 180.0),
    }
}

pub(super) fn focus_cell_bounds(cell_lat: i32, cell_lon: i32) -> GeoBounds {
    GeoBounds {
        min_lat: (cell_lat as f32).clamp(-85.0511, 85.0511),
        max_lat: ((cell_lat + 1) as f32).clamp(-85.0511, 85.0511),
        min_lon: (cell_lon as f32).clamp(-180.0, 180.0),
        max_lon: ((cell_lon + 1) as f32).clamp(-180.0, 180.0),
    }
}

pub(super) fn focus_cell_extract_path(extract_dir: &Path, cell_lat: i32, cell_lon: i32) -> PathBuf {
    extract_dir.join(format!("cell_{:+04}_{:+05}.osm.pbf", cell_lat, cell_lon))
}

pub(super) fn focus_batch_extract_path(
    extract_dir: &Path,
    job_id: i64,
    feature_kind: OsmFeatureKind,
) -> PathBuf {
    extract_dir.join(format!(
        "focus_{}_job_{}.osm.pbf",
        feature_kind.as_str(),
        job_id
    ))
}

pub(super) fn run_osmium_extract(
    osmium: &Path,
    source_path: &Path,
    output_path: &Path,
    bounds: GeoBounds,
) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let bbox = format!(
        "{},{},{},{}",
        bounds.min_lon, bounds.min_lat, bounds.max_lon, bounds.max_lat
    );
    let status = Command::new(osmium)
        .arg("extract")
        .arg("-b")
        .arg(&bbox)
        .arg(source_path)
        .arg("-o")
        .arg(output_path)
        .arg("--overwrite")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("Failed to launch osmium: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("osmium extract failed for bbox {bbox}"))
    }
}

pub(super) fn focus_cell_cached(
    connection: &Connection,
    feature_kind: OsmFeatureKind,
    source_path: &str,
    cell_lat: i32,
    cell_lon: i32,
) -> rusqlite::Result<bool> {
    let count: usize = connection.query_row(
        "SELECT COUNT(*) FROM osm_focus_cell_cache
         WHERE feature_kind = ?1 AND source_path = ?2 AND cell_lat = ?3 AND cell_lon = ?4",
        params![
            feature_kind.as_str(),
            source_path,
            i64::from(cell_lat),
            i64::from(cell_lon),
        ],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub(super) fn mark_focus_cell_cached(
    db_path: &Path,
    feature_kind: OsmFeatureKind,
    source_path: &str,
    cell_lat: i32,
    cell_lon: i32,
) -> Result<(), String> {
    let connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT OR REPLACE INTO osm_focus_cell_cache (
                feature_kind, source_path, cell_lat, cell_lon, imported_at_unix
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                feature_kind.as_str(),
                source_path,
                i64::from(cell_lat),
                i64::from(cell_lon),
                unix_timestamp(),
            ],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}
