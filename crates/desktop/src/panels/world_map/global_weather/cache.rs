//! Stamped global frames, with independent budgets for playback and regional detail.
use super::{Product, provider::MAX_BYTES};
use rusqlite::{Connection, OpenFlags, params};
use std::path::Path;

pub(super) fn read(path: &Path, product: Product) -> Option<(String, Vec<u8>)> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    db.query_row(
        "SELECT stamp, png FROM frames WHERE product=?1 AND length(png)<=?2",
        params![product.id(), MAX_BYTES],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .ok()
}

fn database(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    }
    let db = Connection::open(path)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS frames (
            product TEXT PRIMARY KEY, stamp TEXT NOT NULL, png BLOB NOT NULL);
         CREATE TABLE IF NOT EXISTS history (
            product TEXT NOT NULL, stamp TEXT NOT NULL, png BLOB NOT NULL, used INTEGER NOT NULL,
            PRIMARY KEY(product,stamp));",
    )?;
    Ok(db)
}

fn trim_history(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM history WHERE rowid IN (
            SELECT id FROM (SELECT rowid AS id, row_number() OVER recent AS n,
                sum(length(png)) OVER recent AS total FROM history
                WINDOW recent AS (ORDER BY used DESC, rowid DESC ROWS UNBOUNDED PRECEDING))
            WHERE n>80 OR total>?1)",
        [512_u64 << 20],
    )?;
    Ok(())
}

pub(super) fn write(
    path: &Path,
    product: Product,
    stamp: &str,
    png: &[u8],
) -> rusqlite::Result<()> {
    let mut db = database(path)?;
    let tx = db.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO history SELECT product,stamp,png,unixepoch() FROM frames
            WHERE product=?1 AND stamp<?2",
        params![product.id(), stamp],
    )?;
    tx.execute(
        "INSERT INTO frames (product, stamp, png) VALUES (?1,?2,?3)
        ON CONFLICT(product) DO UPDATE SET stamp=excluded.stamp, png=excluded.png
        WHERE excluded.stamp >= frames.stamp",
        params![product.id(), stamp, png],
    )?;
    trim_history(&tx)?;
    tx.commit()
}

pub(super) fn cached_times(path: &Path, product: Product) -> Vec<String> {
    let Ok(db) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return Vec::new();
    };
    // A cache from an earlier version can contain just the live table.
    let mut stamps = Vec::new();
    for table in ["frames", "history"] {
        if let Ok(mut q) = db.prepare(&format!("SELECT stamp FROM {table} WHERE product=?1"))
            && let Ok(rows) = q.query_map([product.id()], |r| r.get::<_, String>(0))
        {
            stamps.extend(rows.flatten());
        }
    }
    stamps.sort();
    stamps.dedup();
    stamps
}

pub(super) fn historical_png(path: &Path, product: Product, stamp: &str) -> Option<Vec<u8>> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    for table in ["history", "frames"] {
        if let Ok(bytes) = db.query_row(
            &format!("SELECT png FROM {table} WHERE product=?1 AND stamp=?2 AND length(png)<=?3"),
            params![product.id(), stamp, MAX_BYTES],
            |r| r.get(0),
        ) {
            return Some(bytes);
        }
    }
    None
}

pub(super) fn store_history(
    path: &Path,
    product: Product,
    stamp: &str,
    png: &[u8],
) -> rusqlite::Result<()> {
    let mut db = database(path)?;
    let tx = db.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO history VALUES (?1,?2,?3,unixepoch())",
        params![product.id(), stamp, png],
    )?;
    trim_history(&tx)?;
    tx.commit()
}

// Regional PNGs are optional. The last good whole-Earth frames occupy their
// own table and never participate in this eviction policy.
pub(super) fn detail_png(path: &Path, key: &str) -> Option<Vec<u8>> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).ok()?;
    let bytes = db
        .query_row(
            "SELECT png FROM detail WHERE key=?1 AND length(png)<=?2",
            params![key, MAX_BYTES],
            |r| r.get(0),
        )
        .ok()?;
    let _ = db.execute("UPDATE detail SET used=unixepoch() WHERE key=?1", [key]);
    Some(bytes)
}

pub(super) fn store_detail(path: &Path, key: &str, bytes: &[u8]) -> rusqlite::Result<()> {
    let mut db = Connection::open(path)?;
    let tx = db.transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS detail (key TEXT UNIQUE, used INTEGER, png BLOB)",
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO detail VALUES (?1,unixepoch(),?2)",
        params![key, bytes],
    )?;
    tx.execute(
        "DELETE FROM detail WHERE rowid IN (
        SELECT id FROM (SELECT rowid AS id, row_number() OVER recent AS n,
            sum(length(png)) OVER recent AS total FROM detail
            WINDOW recent AS (ORDER BY used DESC, rowid DESC ROWS UNBOUNDED PRECEDING))
        WHERE n>64 OR total>?1)",
        [96_u64 << 20],
    )?;
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn playback_is_exact_and_survives_live_replacement_and_old_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("weather.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE frames(product TEXT PRIMARY KEY, stamp TEXT, png BLOB);
            INSERT INTO frames VALUES ('viirs-noaa20','2026-11-06',X'06')",
        )
        .unwrap();
        let p = Product::TrueColor;
        assert_eq!(cached_times(&path, p), ["2026-11-06"]);
        write(&path, p, "2026-11-08", &[8]).unwrap();
        store_history(&path, p, "2026-11-07", &[7]).unwrap();
        assert_eq!(read(&path, p).unwrap().1, [8]);
        assert_eq!(historical_png(&path, p, "2026-11-06").unwrap(), [6]);
        assert_eq!(
            cached_times(&path, p),
            ["2026-11-06", "2026-11-07", "2026-11-08"]
        );
        assert!(historical_png(&path, p, "2026-11-05").is_none());
    }
    #[test]
    fn delayed_writes_preserve_newer_pixels_and_each_sources_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("weather.sqlite");
        write(&path, Product::TrueColor, "2026-11-08", b"new").unwrap();
        write(&path, Product::TrueColor, "2026-11-07", b"old").unwrap();
        write(
            &path,
            Product::Infrared,
            "2026-11-09T12:00:00Z",
            b"infrared",
        )
        .unwrap();
        assert_eq!(
            read(&path, Product::TrueColor).unwrap(),
            ("2026-11-08".into(), b"new".to_vec())
        );
        assert_eq!(read(&path, Product::Infrared).unwrap().1, b"infrared");
    }
}
