use super::{CACHE_DB_NAME, TileKey};
use crate::terrain_assets;
use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn open_cache_db(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(30))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "temp_store", "MEMORY")?;
    ensure_cache_schema_with_connection(&connection)?;
    Ok(connection)
}

pub fn ensure_cache_schema_with_connection(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS contour_tile_manifest (
            zoom_bucket INTEGER NOT NULL,
            lat_bucket INTEGER NOT NULL,
            lon_bucket INTEGER NOT NULL,
            contour_count INTEGER NOT NULL,
            built_at INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY (zoom_bucket, lat_bucket, lon_bucket)
        );

        CREATE TABLE IF NOT EXISTS contour_tiles (
            zoom_bucket INTEGER NOT NULL,
            lat_bucket INTEGER NOT NULL,
            lon_bucket INTEGER NOT NULL,
            fid INTEGER NOT NULL,
            elevation_m REAL NOT NULL,
            geom BLOB NOT NULL,
            PRIMARY KEY (zoom_bucket, lat_bucket, lon_bucket, fid)
        );

        CREATE INDEX IF NOT EXISTS idx_contour_tiles_lookup
            ON contour_tiles (zoom_bucket, lat_bucket, lon_bucket, elevation_m, fid);

        CREATE TABLE IF NOT EXISTS coastline_tile_manifest (
            zoom_bucket INTEGER NOT NULL,
            lat_bucket INTEGER NOT NULL,
            lon_bucket INTEGER NOT NULL,
            line_count INTEGER NOT NULL,
            built_at INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY (zoom_bucket, lat_bucket, lon_bucket)
        );

        CREATE TABLE IF NOT EXISTS coastline_tiles (
            zoom_bucket INTEGER NOT NULL,
            lat_bucket INTEGER NOT NULL,
            lon_bucket INTEGER NOT NULL,
            fid INTEGER NOT NULL,
            geom BLOB NOT NULL,
            PRIMARY KEY (zoom_bucket, lat_bucket, lon_bucket, fid)
        );

        CREATE INDEX IF NOT EXISTS idx_coastline_tiles_lookup
            ON coastline_tiles (zoom_bucket, lat_bucket, lon_bucket, fid);
        ",
    )?;
    Ok(())
}

pub fn tile_exists(connection: &Connection, tile: TileKey) -> rusqlite::Result<bool> {
    connection
        .query_row(
            "SELECT 1
             FROM contour_tile_manifest
             WHERE zoom_bucket = ?1 AND lat_bucket = ?2 AND lon_bucket = ?3
             LIMIT 1",
            params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
            |_row| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
}

/// Return the contour count recorded for this tile, or `None` if the tile is
/// not in the manifest at all.  A count of 0 means the tile was explicitly
/// marked empty (no source coverage at build time) and can be upgraded when
/// better data (e.g. MOLA) becomes available.
pub fn tile_contour_count(connection: &Connection, tile: TileKey) -> rusqlite::Result<Option<i64>> {
    connection
        .query_row(
            "SELECT contour_count
             FROM contour_tile_manifest
             WHERE zoom_bucket = ?1 AND lat_bucket = ?2 AND lon_bucket = ?3
             LIMIT 1",
            params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
            |row| row.get::<_, i64>(0),
        )
        .optional()
}

pub fn import_tile_into_cache(
    cache_db_path: &Path,
    tile: TileKey,
    gpkg_path: &Path,
) -> rusqlite::Result<()> {
    let source = Connection::open(gpkg_path)?;
    source.busy_timeout(Duration::from_secs(30))?;
    let mut cache = open_cache_db(cache_db_path)?;
    let transaction = cache.transaction()?;
    transaction.execute(
        "DELETE FROM contour_tiles
         WHERE zoom_bucket = ?1 AND lat_bucket = ?2 AND lon_bucket = ?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    transaction.execute(
        "DELETE FROM contour_tile_manifest
         WHERE zoom_bucket = ?1 AND lat_bucket = ?2 AND lon_bucket = ?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;

    let table_exists: bool = source
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='contour'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);

    let mut contour_count = 0usize;
    if table_exists {
        if let Ok(mut statement) = source.prepare("SELECT fid, geom, elevation_m FROM contour ORDER BY ABS(elevation_m), fid") {
            if let Ok(mut rows) = statement.query([]) {
                while let Some(row) = rows.next()? {
                    let fid: i64 = row.get(0)?;
                    let geometry: Vec<u8> = row.get(1)?;
                    let elevation_m: f32 = row.get(2)?;
                    transaction.execute(
                        "INSERT INTO contour_tiles (
                             zoom_bucket,
                             lat_bucket,
                             lon_bucket,
                             fid,
                             elevation_m,
                             geom
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            tile.zoom_bucket,
                            tile.lat_bucket,
                            tile.lon_bucket,
                            fid,
                            elevation_m,
                            geometry
                        ],
                    )?;
                    contour_count += 1;
                }
            }
        }
    }

    transaction.execute(
        "INSERT INTO contour_tile_manifest (
             zoom_bucket,
             lat_bucket,
             lon_bucket,
             contour_count,
             built_at
         ) VALUES (?1, ?2, ?3, ?4, unixepoch())",
        params![
            tile.zoom_bucket,
            tile.lat_bucket,
            tile.lon_bucket,
            contour_count as i64
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

pub fn import_coastline_into_cache(
    cache_db_path: &Path,
    tile: TileKey,
    gpkg_path: &Path,
) -> rusqlite::Result<()> {
    let source = Connection::open(gpkg_path)?;
    source.busy_timeout(Duration::from_secs(30))?;
    // The gpkg may not have the contour table if no 0m crossings exist
    let table_exists: bool = source
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='contour'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);

    let mut cache = open_cache_db(cache_db_path)?;
    let transaction = cache.transaction()?;
    transaction.execute(
        "DELETE FROM coastline_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    transaction.execute(
        "DELETE FROM coastline_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;

    let mut line_count = 0usize;
    if table_exists {
        let mut stmt = source.prepare("SELECT fid, geom FROM contour ORDER BY fid")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let fid: i64 = row.get(0)?;
            let geometry: Vec<u8> = row.get(1)?;
            transaction.execute(
                "INSERT INTO coastline_tiles (zoom_bucket, lat_bucket, lon_bucket, fid, geom)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    tile.zoom_bucket,
                    tile.lat_bucket,
                    tile.lon_bucket,
                    fid,
                    geometry
                ],
            )?;
            line_count += 1;
        }
    }

    transaction.execute(
        "INSERT INTO coastline_tile_manifest (zoom_bucket, lat_bucket, lon_bucket, line_count)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            tile.zoom_bucket,
            tile.lat_bucket,
            tile.lon_bucket,
            line_count as i64
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Store a manifest entry for a tile that has zero contours (e.g. a region with
/// no CTX coverage) so the build is not retried every frame.
pub fn mark_tile_empty(cache_db_path: &Path, tile: TileKey) -> rusqlite::Result<()> {
    let mut cache = open_cache_db(cache_db_path)?;
    let tx = cache.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO contour_tile_manifest
             (zoom_bucket, lat_bucket, lon_bucket, contour_count, built_at)
         VALUES (?1, ?2, ?3, 0, unixepoch())",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    tx.commit()
}

pub fn focus_cache_root(selected_root: Option<&Path>) -> Option<PathBuf> {
    let root = terrain_assets::find_derived_root(selected_root)
        .unwrap_or_else(|| std::env::temp_dir().join("1kee-derived"));
    let cache_root = root.join("terrain");
    fs::create_dir_all(&cache_root).ok()?;
    Some(cache_root)
}

pub fn focus_cache_db_path(selected_root: Option<&Path>) -> Option<PathBuf> {
    Some(focus_cache_root(selected_root)?.join(CACHE_DB_NAME))
}

pub fn journal_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| format!("{}-journal", name.to_string_lossy()))
        .unwrap_or_else(|| "cache.tmp.gpkg-journal".to_string());
    path.with_file_name(file_name)
}

pub fn wal_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| format!("{}-wal", name.to_string_lossy()))
        .unwrap_or_else(|| "cache.tmp.gpkg-wal".to_string());
    path.with_file_name(file_name)
}

pub fn shm_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| format!("{}-shm", name.to_string_lossy()))
        .unwrap_or_else(|| "cache.tmp.gpkg-shm".to_string());
    path.with_file_name(file_name)
}

pub fn cleanup_temp_tile_artifacts(tif_path: &Path, gpkg_path: &Path) {
    let _ = fs::remove_file(tif_path);
    let _ = fs::remove_file(gpkg_path);
    let _ = fs::remove_file(journal_path_for(gpkg_path));
    let _ = fs::remove_file(wal_path_for(gpkg_path));
    let _ = fs::remove_file(shm_path_for(gpkg_path));
}

pub fn temp_tile_paths(cache_root: &Path, tile: TileKey) -> (PathBuf, PathBuf) {
    let temp_root = cache_root.join(super::TEMP_DIR_NAME);
    let stem = format!(
        "z{}_lat{}_lon{}",
        tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
    );
    (
        temp_root.join(format!("{stem}.tmp.tif")),
        temp_root.join(format!("{stem}.tmp.gpkg")),
    )
}
