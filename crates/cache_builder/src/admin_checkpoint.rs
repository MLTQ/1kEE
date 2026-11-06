//! Durable per-relation geometry and shared node coordinates for planet assembly.
use super::geometry::{AdminRegion, CoordinateResolver};
use crate::util::GeoPoint;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
};

pub(super) struct Checkpoint {
    db: Connection,
}
impl Checkpoint {
    pub fn open(path: &Path, source: &str) -> Result<Self, String> {
        let mut db = Connection::open(path).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS identity (id INTEGER PRIMARY KEY CHECK(id=1), source TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS regions (id INTEGER PRIMARY KEY, level INTEGER NOT NULL, rings INTEGER NOT NULL, encoded BLOB NOT NULL);
            CREATE INDEX IF NOT EXISTS regions_by_level ON regions(level,id);
            CREATE TABLE IF NOT EXISTS coordinates (id INTEGER PRIMARY KEY, lat REAL, lon REAL);")
            .map_err(|e|e.to_string())?;
        let saved: Option<String> = db
            .query_row("SELECT source FROM identity WHERE id=1", [], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        if saved.as_deref() != Some(source) {
            let tx = db.transaction().map_err(|e| e.to_string())?;
            tx.execute_batch("DELETE FROM regions; DELETE FROM coordinates;")
                .map_err(|e| e.to_string())?;
            tx.execute("INSERT OR REPLACE INTO identity VALUES(1,?1)", [source])
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }
        Ok(Self { db })
    }
    pub fn completed(&self, level: u8) -> Result<HashSet<i64>, String> {
        self.db
            .prepare("SELECT id FROM regions WHERE level=?1")
            .and_then(|mut s| s.query_map([level], |r| r.get(0))?.collect())
            .map_err(|e| e.to_string())
    }
    pub fn save(&self, level: u8, region: &AdminRegion) -> Result<(), String> {
        let features = crate::geojson::admin_cell_features(level, std::slice::from_ref(region));
        let bytes = cell_format::write::write_cell(0, 0, &[(cell_format::TAG_ADMN, &features)]);
        self.db
            .execute(
                "INSERT OR REPLACE INTO regions VALUES(?1,?2,?3,?4)",
                params![region.relation_id, level, features.len() as u64, bytes],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    /// Resolve absent coordinates together and persist them for neighboring
    /// relations, finer levels and restarts. A saved NULL is a known missing node.
    pub fn resolve(
        &mut self,
        ids: &[i64],
        underlying: &mut CoordinateResolver<'_>,
    ) -> Result<Vec<Option<GeoPoint>>, String> {
        let mut unique = ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        let mut found: HashMap<i64, Option<GeoPoint>> = HashMap::new();
        for batch in unique.chunks(500) {
            let slots = std::iter::repeat_n("?", batch.len())
                .collect::<Vec<_>>()
                .join(",");
            let mut stmt = self
                .db
                .prepare_cached(&format!(
                    "SELECT id,lat,lon FROM coordinates WHERE id IN ({slots})"
                ))
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params_from_iter(batch), |r| {
                    let lat: Option<f32> = r.get(1)?;
                    let lon: Option<f32> = r.get(2)?;
                    Ok((
                        r.get(0)?,
                        lat.zip(lon).map(|(lat, lon)| GeoPoint { lat, lon }),
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (id, p) = row.map_err(|e| e.to_string())?;
                found.insert(id, p);
            }
        }
        let missing: Vec<_> = unique
            .into_iter()
            .filter(|id| !found.contains_key(id))
            .collect();
        if !missing.is_empty() {
            let points = underlying(&missing)?;
            if points.len() != missing.len() {
                return Err("Admin coordinate lookup changed reference count".into());
            }
            let tx = self.db.transaction().map_err(|e| e.to_string())?;
            {
                let mut insert = tx
                    .prepare_cached("INSERT OR REPLACE INTO coordinates VALUES(?1,?2,?3)")
                    .map_err(|e| e.to_string())?;
                for (id, p) in missing.into_iter().zip(points) {
                    insert
                        .execute(params![id, p.map(|p| p.lat), p.map(|p| p.lon)])
                        .map_err(|e| e.to_string())?;
                    found.insert(id, p);
                }
            }
            tx.commit().map_err(|e| e.to_string())?;
        }
        Ok(ids.iter().map(|id| found[id]).collect())
    }
    /// Stream encoded relation records into the existing single ADMN chunk.
    /// Peak publication memory is one region, independent of a level's size.
    pub fn publish(&self, output: &Path, level: u8) -> Result<usize, String> {
        let (rings,body): (u64,u64) = self.db.query_row("SELECT COALESCE(SUM(rings),0),COALESCE(SUM(length(encoded)-22),0) FROM regions WHERE level=?1",[level],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?;
        let length = u32::try_from(body + 4)
            .map_err(|_| "Boundary level exceeds the 1kc chunk size limit")?;
        let count = u32::try_from(rings).map_err(|_| "Too many boundary rings in one level")?;
        let dir = output.join("admin_cells");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(cell_format::admin_filename(level));
        let temporary = path.with_extension("1kc.tmp");
        let mut header = cell_format::write::write_cell(0, 0, &[(cell_format::TAG_ADMN, &[])]);
        header[14..18].copy_from_slice(&length.to_le_bytes());
        header[18..22].copy_from_slice(&count.to_le_bytes());
        let mut out = BufWriter::new(File::create(&temporary).map_err(|e| e.to_string())?);
        out.write_all(&header).map_err(|e| e.to_string())?;
        let mut stmt = self
            .db
            .prepare("SELECT rings,encoded FROM regions WHERE level=?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query([level]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let n: u32 = row.get(0).map_err(|e| e.to_string())?;
            let bytes: Vec<u8> = row.get(1).map_err(|e| e.to_string())?;
            if bytes.len() < 22
                || bytes[..14] != header[..14]
                || u32::from_le_bytes(bytes[14..18].try_into().unwrap()) as usize
                    != bytes.len() - 18
                || u32::from_le_bytes(bytes[18..22].try_into().unwrap()) != n
            {
                return Err(
                    "Invalid saved administrative geometry; previous published level retained"
                        .into(),
                );
            }
            out.write_all(&bytes[22..]).map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
        out.get_ref().sync_all().map_err(|e| e.to_string())?;
        drop(out);
        fs::rename(temporary, path).map_err(|e| e.to_string())?;
        Ok(rings as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_coordinates_and_missing_nodes_survive_restart_but_not_source_change() {
        let path = std::env::temp_dir().join(format!(
            "admin-coordinates-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let p = GeoPoint {
            lat: 1.25,
            lon: -2.75,
        };
        {
            let mut cache = Checkpoint::open(&path, "source-a").unwrap();
            let points = cache
                .resolve(&[3, 1, 3, 2], &mut |ids| {
                    assert_eq!(ids, &[1, 2, 3]);
                    Ok(vec![Some(p), None, Some(p)])
                })
                .unwrap();
            assert_eq!(points, vec![Some(p), Some(p), Some(p), None]);
            cache
                .save(
                    2,
                    &AdminRegion {
                        relation_id: 2,
                        name: None,
                        rings: vec![],
                        complete: false,
                    },
                )
                .unwrap();
        }
        let mut cache = Checkpoint::open(&path, "source-a").unwrap();
        assert_eq!(
            cache
                .resolve(&[2, 1], &mut |_| panic!("coordinates must be reused"))
                .unwrap(),
            vec![None, Some(p)]
        );
        assert!(cache.completed(2).unwrap().contains(&2));
        drop(cache);
        let mut cache = Checkpoint::open(&path, "source-b").unwrap();
        assert!(cache.completed(2).unwrap().is_empty());
        assert_eq!(
            cache
                .resolve(&[2], &mut |ids| {
                    assert_eq!(ids, &[2]);
                    Ok(vec![Some(p)])
                })
                .unwrap(),
            vec![Some(p)]
        );
        drop(cache);
        fs::remove_file(path).unwrap();
    }
}
