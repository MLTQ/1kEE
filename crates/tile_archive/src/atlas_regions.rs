//! Independently installable 5-degree Atlas relief regions. Pinned files use
//! the normal height schema, are fully verified, and never share a WAL.
use super::{Key, Reader, Tier, Writer, get, plan};
use rusqlite::Connection;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    x: u8,
    y: u8,
}
impl Region {
    pub fn at(lon: f64, lat: f64) -> Self {
        Self {
            x: ((lon + 180.).rem_euclid(360.) / 5.).floor() as u8,
            y: ((lat.clamp(-90., 90. - 1e-9) + 90.) / 5.).floor() as u8,
        }
    }
    pub fn bounds(self) -> [f64; 4] {
        let w = f64::from(self.x) * 5. - 180.;
        let s = f64::from(self.y) * 5. - 90.;
        [w, s, w + 5., s + 5.]
    }
    /// Include neighboring height posts needed at the edge of a Globe tile.
    pub fn source_bounds(self) -> [f64; 4] {
        let [w, s, _, n] = self.bounds();
        let d = 15. / 3600.;
        let west = (w - d + 180.).rem_euclid(360.) - 180.;
        [
            west,
            (s - d).max(-90.),
            west + 5. + 2. * d,
            (n + d).min(90.),
        ]
    }
    pub fn path(self, directory: &Path) -> PathBuf {
        directory.join(format!("atlas-{}-{}.sqlite", self.x, self.y))
    }
    fn keys(self) -> super::Result<Vec<Key>> {
        plan(self.source_bounds(), &[Tier::Terrain])
    }
    pub fn verify(self, path: &Path) -> super::Result<()> {
        let reader = Reader::open(path)?;
        for key in self.keys()? {
            get(&reader.connection, key, None)?
                .ok_or_else(|| format!("Missing regional tile {},{}", key.x, key.y))?;
        }
        Ok(())
    }
    /// Extract only the selected region and its halo, including its original
    /// source fingerprints. One read transaction gives a consistent snapshot.
    pub fn extract(self, source: &Path, dest: &Path) -> super::Result<()> {
        let mut reader = Reader::open(source)?;
        let tx = reader.connection.transaction().map_err(|e| e.to_string())?;
        let writer = Writer::open(dest)?;
        for key in self.keys()? {
            let values = get(&tx, key, None)?
                .ok_or_else(|| format!("Missing regional tile {},{}", key.x, key.y))?;
            let stamp: String = tx
                .query_row(
                    "SELECT source FROM heights WHERE tier=?1 AND x=?2 AND y=?3",
                    (key.tier as i32, key.x, key.y),
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            writer.put(key, &stamp, &values)?;
        }
        writer.finish()
    }
    pub fn install(
        self,
        directory: &Path,
        budget: u64,
        prepare: impl FnOnce(&Path) -> super::Result<()>,
    ) -> super::Result<u64> {
        fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let scratch = tempfile::tempdir_in(directory).map_err(|e| e.to_string())?;
        let stage = scratch.path().join("region.sqlite");
        prepare(&stage)?;
        let db = Connection::open(&stage).map_err(|e| e.to_string())?;
        let mode: String = db
            .query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err("Could not seal regional archive".into());
        }
        drop(db);
        self.verify(&stage)?;
        fs::File::open(&stage)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        let size = stage.metadata().map_err(|e| e.to_string())?.len();
        let dest = self.path(directory);
        let _lock = lock(directory)?;
        let mut used = size;
        for f in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let path = f.map_err(|e| e.to_string())?.path();
            if path != dest && is_region_file(&path) {
                used = used.saturating_add(path.metadata().map_err(|e| e.to_string())?.len());
            }
        }
        if used > budget {
            return Err(format!(
                "Pinned terrain needs {} MiB; budget {} MiB",
                used.div_ceil(1 << 20),
                budget >> 20
            ));
        }
        fs::rename(&stage, dest).map_err(|e| e.to_string())?;
        sync_dir(directory)?;
        Ok(size)
    }
    pub fn unpin(self, directory: &Path) -> super::Result<()> {
        let _lock = lock(directory)?;
        fs::remove_file(self.path(directory)).map_err(|e| e.to_string())?;
        sync_dir(directory)
    }
}
fn is_region_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.starts_with("atlas-") && n.ends_with(".sqlite"))
}
fn lock(directory: &Path) -> super::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("regions.lock"))
        .map_err(|e| e.to_string())?;
    file.lock().map_err(|e| e.to_string())?;
    Ok(file)
}
fn sync_dir(_dir: &Path) -> super::Result<()> {
    #[cfg(unix)]
    fs::File::open(_dir)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn install_verify_cross_date_line_and_keep_good_copy_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let region = Region::at(-179., 89.);
        let src = tmp.path().join("source.sqlite");
        let writer = Writer::open(&src).unwrap();
        for key in region.keys().unwrap() {
            writer
                .put(key, "fixture", &vec![37.; Tier::Terrain.side().pow(2)])
                .unwrap();
        }
        writer.finish().unwrap();
        drop(writer);
        let dst = tmp.path().join("regions");
        region
            .install(&dst, 1 << 26, |p| region.extract(&src, p))
            .unwrap();
        assert!(
            region
                .install(&dst, 1, |p| region.extract(&src, p))
                .is_err()
        );
        assert!(
            region
                .install(&dst, 1 << 26, |p| fs::write(p, b"invalid")
                    .map_err(|e| e.to_string()))
                .is_err()
        );
        region.verify(&region.path(&dst)).unwrap();
        region.unpin(&dst).unwrap();
        assert!(!region.path(&dst).exists());
    }
}
