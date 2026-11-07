//! Discard failed tiles without requiring writes through damaged SQLite pages.
use super::{TileKey, builders, db};
use rusqlite::{Connection, Error, ErrorCode, OptionalExtension, params};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

static REPAIR: Mutex<()> = Mutex::new(());

pub fn corrupt(error: &Error) -> bool {
    matches!(error, Error::SqliteFailure(e, _) if matches!(e.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase))
}

pub fn invalid_geometry(error: &Error) -> bool {
    corrupt(error)
        || matches!(
            error,
            Error::InvalidColumnType(..) | Error::FromSqlConversionFailure(..)
        )
}

pub fn isolated_root() -> Option<PathBuf> {
    std::env::var_os("ONEKEE_TERRAIN_REPAIR_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            crate::settings_store::load_app_settings()
                .terrain_repair_root
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
        })
        .filter(|p| p.is_absolute())
}

fn root_for(path: &Path, isolated: Option<&Path>) -> PathBuf {
    if let Some(isolated) = isolated {
        // Stable path namespace, unlike DefaultHasher's unspecified persistence.
        let hash = path
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .fold(0xcbf29ce484222325u64, |h, b| {
                (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
            });
        isolated
            .join(format!("{hash:016x}"))
            .join(path.file_name().unwrap_or_default())
    } else {
        path.parent()
            .unwrap_or(Path::new("."))
            .join(".tile-repairs")
            .join(path.file_name().unwrap_or_default())
    }
}

fn root(path: &Path) -> PathBuf {
    root_for(path, isolated_root().as_deref())
}

fn replacement(path: &Path, tile: TileKey) -> PathBuf {
    root(path)
        .join(format!(
            "{}_{}_{}",
            tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
        ))
        .join(path.file_name().unwrap_or_default())
}

// The directory itself is the durable invalidation marker. Keep the original
// filename so native-resolution provenance validation continues to apply.
pub fn resolve(path: &Path, tile: TileKey) -> PathBuf {
    let mut path = path.to_path_buf();
    for _ in 0..8 {
        let next = replacement(&path, tile);
        if !next.parent().is_some_and(Path::is_dir) {
            break;
        }
        path = next;
    }
    path
}

pub fn mark_database(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root(path))
}

pub fn build_path(
    path: &Path,
    tile: TileKey,
    count: Option<i64>,
    allow_build: bool,
) -> Option<PathBuf> {
    let resolved = resolve(path, tile);
    if resolved != path {
        return Some(resolved);
    }
    if count.is_none() && allow_build && (isolated_root().is_some() || root(path).is_dir()) {
        let target = replacement(path, tile);
        if let Err(e) = std::fs::create_dir_all(target.parent()?) {
            eprintln!("[1kEE] Cannot create terrain replacement: {e}");
            return None;
        }
        return Some(target);
    }
    Some(resolved)
}

pub fn count(path: &Path, tile: TileKey) -> rusqlite::Result<Option<i64>> {
    if !path.exists() {
        return Ok(None);
    }
    let connection = db::open_cache_db_read_only(path)?;
    connection.query_row(
        "SELECT contour_count FROM contour_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket], |r| r.get(0),
    ).optional()
}

fn delete_tile(path: &Path, tile: TileKey) -> rusqlite::Result<bool> {
    let mut connection = db::open_cache_db(path)?;
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // Recheck while holding the writer lock: another reader may have already
    // discarded this tile and a builder may have committed its replacement.
    if readable(&tx, tile)? {
        return Ok(false);
    }
    for table in [
        "contour_tiles",
        "contour_tile_manifest",
        "coastline_tiles",
        "coastline_tile_manifest",
    ] {
        tx.execute(
            &format!(
                "DELETE FROM {table} WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3"
            ),
            params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
        )?;
    }
    tx.commit()?;
    Ok(true)
}

pub fn readable(connection: &Connection, tile: TileKey) -> rusqlite::Result<bool> {
    let expected: Option<i64> = connection.query_row(
        "SELECT contour_count FROM contour_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket], |r| r.get(0),
    ).optional()?;
    let Some(expected) = expected else {
        return Ok(true);
    }; // already invalidated
    let mut statement = connection.prepare("SELECT geom,elevation_m FROM contour_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3")?;
    let mut rows = statement.query(params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket])?;
    let mut count = 0i64;
    while let Some(row) = rows.next()? {
        let geometry = match row.get_ref(0).and_then(|v| v.as_blob().map_err(Into::into)) {
            Ok(blob) => blob,
            Err(_) => return Ok(false),
        };
        let elevation = match row.get::<_, f64>(1) {
            Ok(v) => v,
            Err(_) => return Ok(false),
        };
        if !elevation.is_finite()
            || tile_archive::gpkg::try_parse_gpkg_lines(geometry, |_, _| ()).is_none()
        {
            return Ok(false);
        }
        count += 1;
    }
    Ok(count == expected)
}

/// Called only by background readers after dropping their SQLite snapshots.
/// A failed delete rolls back. Corrupt databases retain other readable tiles;
/// only this key is shadowed by a fresh per-tile database on subsequent reads.
pub fn discard(path: &Path, tile: TileKey) -> Result<(), String> {
    let _guard = REPAIR.lock().map_err(|e| e.to_string())?;
    if resolve(path, tile) != path {
        return Ok(());
    }
    let outcome = if isolated_root().is_some_and(|root| !path.starts_with(root)) {
        // The source volume may reject writes: do not open it writable, even
        // for a rollback-only attempt. Shadow only a confirmed unreadable tile.
        let current = db::open_cache_db_read_only(path).and_then(|c| readable(&c, tile));
        match current {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) if invalid_geometry(&e) => {}
            Err(e) => return Err(e.to_string()),
        }
        Err(Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            None,
        ))
    } else {
        delete_tile(path, tile)
    };
    match outcome {
        Ok(false) => return Ok(()),
        Ok(true) => eprintln!(
            "[1kEE] Discarded unreadable terrain tile {}/{}/{} in {}; regeneration queued",
            tile.zoom_bucket,
            tile.lat_bucket,
            tile.lon_bucket,
            path.display()
        ),
        Err(error) if corrupt(&error) => {
            let target = replacement(path, tile);
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            eprintln!(
                "[1kEE] Isolated unreadable tile from {}; invalidated {}/{}/{}; regeneration will use {}",
                path.display(),
                tile.zoom_bucket,
                tile.lat_bucket,
                tile.lon_bucket,
                target.display()
            );
        }
        Err(error) => return Err(error.to_string()),
    }
    builders::clear_failed_build_backoffs();
    builders::bump_manifest_revision();
    crate::app::request_repaint();
    Ok(())
}

pub fn report(path: &Path, tile: TileKey) {
    if let Err(e) = discard(path, tile) {
        eprintln!(
            "[1kEE] Could not invalidate terrain tile {}/{}/{}: {e}",
            tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
        );
    }
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;

/// Explicit headless maintenance for one native Earth tile. Also used to verify
/// live repairs without changing the user's viewport or running a planet job.
pub fn repair_native_tile(path: &Path, source: &Path, tile: TileKey) -> Result<PathBuf, String> {
    if path.file_name().and_then(|n| n.to_str())
        != Some(tile_archive::contour_grid::SRTM_NATIVE_DB_NAME)
    {
        return Err(
            "Expected srtm_native_v1.sqlite; hosted/Moon/Mars repairs use the desktop scheduler"
                .into(),
        );
    }
    let zoom = [0.5, 1.5, 2.5, 3.5, 5.5, 8.0, 11.0]
        .get(tile.zoom_bucket as usize)
        .copied()
        .ok_or("Native SRTM bucket must be 0..6")?;
    let spec = super::zoom::spec_for_zoom(zoom);
    let core = tile_archive::contour_grid::CoreTile::srtm(
        spec.half_extent_deg,
        tile.lat_bucket,
        tile.lon_bucket,
    );
    let bounds = super::GeoBounds {
        min_lat: core.source.min_lat as f32,
        max_lat: core.source.max_lat as f32,
        min_lon: core.source.min_lon as f32,
        max_lon: core.source.max_lon as f32,
    };
    if !source.is_dir() || !super::gdal::bounds_have_srtm_source(source, bounds) {
        return Err("No SRTM source tiles found for requested repair".into());
    }
    let current = resolve(path, tile);
    let valid = db::open_cache_db_read_only(&current).and_then(|c| {
        let present = db::tile_exists(&c, tile)?;
        readable(&c, tile).map(|valid| present && valid)
    });
    match valid {
        Ok(true) => return Ok(current),
        Ok(false) => {
            discard(&current, tile)?;
        }
        Err(e) if invalid_geometry(&e) => {
            discard(&current, tile)?;
        }
        Err(e) if !current.exists() && current != path => {
            let _ = e;
        }
        Err(e) => return Err(e.to_string()),
    }
    let target = build_path(path, tile, None, true).ok_or("Could not create replacement path")?;
    let cache_root = target.parent().ok_or("Cache has no parent")?;
    super::gdal::build_focus_contours(source, cache_root, &target, tile, bounds, spec)
        .ok_or("Tile regeneration failed; it remains invalid and may retry")?;
    let c = db::open_cache_db_read_only(&target).map_err(|e| e.to_string())?;
    if !db::tile_exists(&c, tile).map_err(|e| e.to_string())?
        || !readable(&c, tile).map_err(|e| e.to_string())?
    {
        return Err("Regenerated tile failed validation".into());
    }
    Ok(target)
}
