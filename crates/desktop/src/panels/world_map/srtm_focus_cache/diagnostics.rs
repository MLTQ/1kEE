//! Read-only, bounded cache diagnostics. Called on a Settings worker, not paint.
use super::{TileKey, db, recovery};
use rusqlite::{OptionalExtension, params};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct Level {
    pub zoom: i32,
    pub tiles: i64,
    pub lines: i64,
}

#[derive(Debug)]
pub(crate) enum Original {
    Ready(Vec<Level>),
    Missing,
    Unreadable(String),
}

#[derive(Default, Debug)]
pub(crate) enum Recovery {
    #[default]
    Absent,
    Present,
    Available {
        sample: PathBuf,
    },
    Unreadable(String),
}

#[derive(Debug)]
pub(crate) struct Cache {
    pub label: &'static str,
    pub path: PathBuf,
    pub original: Original,
    pub recovery_root: PathBuf,
    pub recovery: Recovery,
}

pub(crate) fn load(root: Option<&Path>) -> Vec<Cache> {
    let directory = db::focus_cache_directory(root);
    [
        (
            "SRTM contours",
            tile_archive::contour_grid::SRTM_NATIVE_DB_NAME,
        ),
        ("Detailed contours", super::CACHE_DB_NAME),
    ]
    .into_iter()
    .map(|(label, name)| {
        let path = directory.join(name);
        inspect(label, &path, recovery::root(&path), |tile| {
            recovery::resolve(&path, tile)
        })
    })
    .collect()
}

fn original(path: &Path) -> Original {
    // An error is never a zero count. Do not create a cache just to inspect it.
    match fs::metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Original::Missing,
        Err(error) => return Original::Unreadable(error.to_string()),
    }
    let result = (|| -> rusqlite::Result<Vec<Level>> {
        let connection = db::open_cache_db_read_only(path)?;
        let mut statement = connection.prepare(
            "SELECT zoom_bucket, COUNT(*), COALESCE(SUM(contour_count),0) \
             FROM contour_tile_manifest GROUP BY zoom_bucket ORDER BY zoom_bucket",
        )?;
        statement
            .query_map([], |row| {
                Ok(Level {
                    zoom: row.get(0)?,
                    tiles: row.get(1)?,
                    lines: row.get(2)?,
                })
            })?
            .collect()
    })();
    match result {
        Ok(levels) => Original::Ready(levels),
        Err(error) => Original::Unreadable(error.to_string()),
    }
}

fn tile(name: &str) -> Option<TileKey> {
    let mut parts = name.split('_');
    let tile = TileKey {
        zoom_bucket: parts.next()?.parse().ok()?,
        lat_bucket: parts.next()?.parse().ok()?,
        lon_bucket: parts.next()?.parse().ok()?,
    };
    parts.next().is_none().then_some(tile)
}

fn recovered(base: &Path, root: &Path, resolve: impl Fn(TileKey) -> PathBuf) -> Recovery {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Recovery::Absent,
        Err(error) => return Recovery::Unreadable(error.to_string()),
    };
    // Thousands of individual replacement databases may exist. Verify at most
    // 32 locations, without claiming a total or performing a whole-cache scan.
    for entry in entries.take(32).flatten() {
        let Some(tile) = entry.file_name().to_str().and_then(tile) else {
            continue;
        };
        let path = resolve(tile);
        if path == base || !path.is_file() {
            continue;
        }
        let valid = db::open_cache_db_read_only(&path).and_then(|connection| {
            connection
                .query_row(
                    "SELECT contour_count FROM contour_tile_manifest \
                 WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
                    params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
        });
        if matches!(valid, Ok(Some(count)) if count >= 0) {
            return Recovery::Available { sample: path };
        }
    }
    Recovery::Present
}

fn inspect(
    label: &'static str,
    path: &Path,
    recovery_root: PathBuf,
    resolve: impl Fn(TileKey) -> PathBuf,
) -> Cache {
    Cache {
        label,
        path: path.to_path_buf(),
        original: original(path),
        recovery: recovered(path, &recovery_root, resolve),
        recovery_root,
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
