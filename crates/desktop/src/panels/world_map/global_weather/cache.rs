//! One last good PNG per global product. Replace its pixels and time together.
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

pub(super) fn write(path: &Path, product: Product, stamp: &str, png: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let db = Connection::open(path).map_err(|e| e.to_string())?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS frames (
        product TEXT PRIMARY KEY, stamp TEXT NOT NULL, png BLOB NOT NULL
    )",
    )
    .map_err(|e| e.to_string())?;
    db.execute(
        "INSERT INTO frames (product, stamp, png) VALUES (?1,?2,?3)
        ON CONFLICT(product) DO UPDATE SET stamp=excluded.stamp, png=excluded.png
        WHERE excluded.stamp >= frames.stamp",
        params![product.id(), stamp, png],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
