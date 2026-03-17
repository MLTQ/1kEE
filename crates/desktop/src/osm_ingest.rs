use crate::model::GeoPoint;
use osmpbf::{BlobDecode, BlobReader, Element, ElementReader};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

const PLANET_PBF_NAME: &str = "planet-latest.osm.pbf";
const RUNTIME_DB_NAME: &str = "osm_runtime.sqlite";
const PLANET_ROADS_NOTE: &str = "planet_roads_bootstrap_v1";
const ROAD_TILE_ZOOMS: &[u8] = &[4, 6, 8, 10];
const PROGRESS_FLUSH_INTERVAL: usize = 25_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsmFeatureKind {
    Roads,
    Buildings,
}

impl OsmFeatureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Roads => "roads",
            Self::Buildings => "buildings",
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
    pub primary_runtime_source: &'static str,
}

#[derive(Clone)]
pub struct OsmJobSnapshot {
    pub label: String,
    pub state: String,
    pub note: String,
}

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

        let (runtime_db_ready, queued_jobs, road_tiles, building_tiles) = runtime_db_path
            .as_ref()
            .filter(|path| path.exists())
            .and_then(|path| read_runtime_counts(path).ok())
            .unwrap_or((false, 0, 0, 0));

        let primary_runtime_source = if road_tiles > 0 || building_tiles > 0 {
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

    Ok(db_path)
}

pub fn queue_region_job(
    selected_root: Option<&Path>,
    feature_kind: OsmFeatureKind,
    bounds: GeoBounds,
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
                min_lat, max_lat, min_lon, max_lon,
                requested_at_unix, updated_at_unix, note
             ) VALUES (?1, 'queued', ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)",
            params![
                feature_kind.as_str(),
                source_path.display().to_string(),
                bounds.min_lat,
                bounds.max_lat,
                bounds.min_lon,
                bounds.max_lon,
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
        Some(PLANET_ROADS_NOTE),
    )?;
    Ok(true)
}

pub fn tick(selected_root: Option<&Path>) {
    let Ok(db_path) = ensure_runtime_store(selected_root) else {
        return;
    };

    let worker = worker();
    let mut guard = match worker.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    if let Some(active) = guard.as_ref() {
        if active.handle.is_finished() {
            let active = guard.take().expect("finished worker present");
            drop(guard);
            let _ = active.handle.join();
        } else {
            return;
        }
    } else {
        drop(guard);
    }

    let connection = match open_runtime_db(&db_path) {
        Ok(connection) => connection,
        Err(_) => return,
    };
    let Some(job) = fetch_next_job(&connection).ok().flatten() else {
        return;
    };
    drop(connection);

    let handle = thread::spawn(move || {
        let result = match job.feature_kind {
            OsmFeatureKind::Roads => import_planet_roads(&db_path, &job),
            OsmFeatureKind::Buildings => {
                Err("Planet building import is not implemented yet.".to_owned())
            }
        };

        match result {
            Ok(summary) => {
                let _ = mark_job_completed(&db_path, job.id, &summary);
            }
            Err(error) => {
                let _ = mark_job_failed(&db_path, job.id, &error);
            }
        }
    });

    if let Ok(mut guard) = worker.lock() {
        *guard = Some(ActiveWorker { handle });
    }
}

pub fn has_active_jobs(selected_root: Option<&Path>) -> bool {
    snapshots(selected_root)
        .iter()
        .any(|job| matches!(job.state.as_str(), "queued" | "running"))
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
                "roads" => "Road region import".to_owned(),
                "buildings" => "Building import".to_owned(),
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
    if let Some(root) = selected_root {
        if let Some(path) = find_planet_from(root) {
            return Some(path);
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        if let Some(path) = find_planet_from(&cwd) {
            return Some(path);
        }
    }

    [
        PathBuf::from("/Volumes/Hilbert/Data/planet-latest.osm.pbf"),
        PathBuf::from("/Volumes/Hilbert/Data").join(PLANET_PBF_NAME),
    ]
    .into_iter()
    .find(|candidate| candidate.exists())
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
            ON building_tiles(zoom, tile_x, tile_y);",
    )?;
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
         ORDER BY requested_at_unix ASC
         LIMIT 1",
    )?;

    let job = statement
        .query_row([], |row| {
            let feature_kind = match row.get::<_, String>(1)?.as_str() {
                "roads" => OsmFeatureKind::Roads,
                "buildings" => OsmFeatureKind::Buildings,
                _ => OsmFeatureKind::Roads,
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
            self.connection.execute(
                "INSERT OR REPLACE INTO road_tile_manifest (
                    zoom, tile_x, tile_y, feature_count, built_at_unix
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    i64::from(zoom),
                    i64::from(tile_x),
                    i64::from(tile_y),
                    feature_count as i64,
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

fn lat_lon_to_tile(lat: f32, lon: f32, zoom: u8) -> (u32, u32) {
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

fn read_runtime_counts(path: &Path) -> rusqlite::Result<(bool, usize, usize, usize)> {
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

    Ok((true, queued_jobs, road_tiles, building_tiles))
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
