//! Prevent coarse source caches from being reused as native SRTM geometry.
use rusqlite::{Connection, OptionalExtension};

const QUALITY: &str = "srtm-gl1-native-v1";

pub fn is_native(db: &Connection) -> rusqlite::Result<bool> {
    let table: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='contour_source_metadata')",
        [],
        |row| row.get(0),
    )?;
    if !table {
        return Ok(false);
    }
    let value: Option<String> = db
        .query_row(
            "SELECT value FROM contour_source_metadata WHERE key='quality'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value.as_deref() == Some(QUALITY))
}

/// Call after schema initialization and before planning any native builds.
/// Existing untagged data requires a separate destination, never an overwrite.
pub fn initialize(db: &Connection) -> rusqlite::Result<()> {
    if is_native(db)? {
        return Ok(());
    }
    let occupied: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM contour_tile_manifest) OR EXISTS(SELECT 1 FROM coastline_tile_manifest)",
        [], |row| row.get(0),
    )?;
    if occupied {
        return Err(rusqlite::Error::ToSqlConversionFailure(std::io::Error::other(
            "This cache contains older terrain geometry. Choose a new srtm_native_v1.sqlite destination; existing data will not be replaced.",
        ).into()));
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS contour_source_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)")?;
    db.execute(
        "INSERT OR REPLACE INTO contour_source_metadata VALUES('quality',?1)",
        [QUALITY],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_cache_is_tagged_but_legacy_manifests_are_never_relabelled() {
        for occupied in [false, true] {
            let db = Connection::open_in_memory().unwrap();
            db.execute_batch("CREATE TABLE contour_tile_manifest(id INTEGER); CREATE TABLE coastline_tile_manifest(id INTEGER);").unwrap();
            if occupied {
                db.execute("INSERT INTO contour_tile_manifest VALUES(1)", [])
                    .unwrap();
            }
            assert!(!is_native(&db).unwrap());
            assert_eq!(initialize(&db).is_ok(), !occupied);
            assert_eq!(is_native(&db).unwrap(), !occupied);
            if !occupied {
                db.execute("INSERT INTO contour_tile_manifest VALUES(1)", [])
                    .unwrap();
                initialize(&db).unwrap();
            }
            assert_eq!(
                db.query_row("SELECT COUNT(*) FROM contour_tile_manifest", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }
}
