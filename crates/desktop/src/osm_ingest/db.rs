use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

use super::{GeoBounds, OsmFeatureKind, OsmJob, OsmJobSnapshot};
use super::util::unix_timestamp;

pub(super) fn runtime_db_path(selected_root: Option<&Path>) -> Option<PathBuf> {
    let derived_root = crate::terrain_assets::find_derived_root(selected_root)?;
    Some(derived_root.join("osm").join(super::RUNTIME_DB_NAME))
}

pub(super) fn open_runtime_db(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(connection)
}

pub(super) fn ensure_runtime_schema(connection: &Connection) -> rusqlite::Result<()> {
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
            ON water_tiles(zoom, tile_x, tile_y);
        CREATE TABLE IF NOT EXISTS osm_focus_cell_cache (
            feature_kind TEXT NOT NULL,
            source_path TEXT NOT NULL,
            cell_lat INTEGER NOT NULL,
            cell_lon INTEGER NOT NULL,
            imported_at_unix INTEGER NOT NULL,
            PRIMARY KEY (feature_kind, source_path, cell_lat, cell_lon)
        );
        CREATE INDEX IF NOT EXISTS idx_osm_focus_cell_cache_lookup
            ON osm_focus_cell_cache(feature_kind, source_path);",
    )?;
    let _ = connection.execute(
        "ALTER TABLE osm_ingest_jobs ADD COLUMN priority INTEGER NOT NULL DEFAULT 0",
        [],
    );
    Ok(())
}

pub(super) fn fetch_next_job(connection: &Connection) -> rusqlite::Result<Option<OsmJob>> {
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

pub(super) fn mark_job_completed(db_path: &Path, job_id: i64, note: &str) -> Result<(), String> {
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

pub(super) fn mark_job_failed(db_path: &Path, job_id: i64, error_text: &str) -> Result<(), String> {
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

pub(super) fn update_job_note(db_path: &Path, job_id: i64, note: &str) -> Result<(), String> {
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

pub(super) fn has_job_note(selected_root: Option<&Path>, note: &str) -> Result<bool, String> {
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

pub(super) fn recover_orphaned_running_jobs(db_path: &Path) -> Result<(), String> {
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

pub(super) fn register_planet_source(connection: &Connection, path: &Path) -> rusqlite::Result<()> {
    use std::fs;
    let metadata = fs::metadata(path)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(super::util::system_time_to_unix)
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

pub(super) fn read_runtime_counts(path: &Path) -> rusqlite::Result<(bool, usize, usize, usize, usize)> {
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

pub(super) fn job_snapshots(
    selected_root: Option<&Path>,
    planet_roads_note: &str,
    focus_roads_note_prefix: &str,
) -> Vec<OsmJobSnapshot> {
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
                "roads" if note == planet_roads_note => "Global roads bootstrap".to_owned(),
                "roads" if note.starts_with(focus_roads_note_prefix) => {
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
