use super::{Result, geometry::*, source};
use cell_format::CellPoint;
use rusqlite::{Connection, params};
use std::fs::File;

fn box_coords(pts: &[CellPoint]) -> [f32; 4] {
    let [w, s, e, n] = bounds(pts);
    let shift = 360. * (((w + e) * 0.5) / 360.).round();
    [w - shift, s, e - shift, n]
}

pub(super) fn populate(db: &Connection, file: &mut File, level: u8, identity: &str) -> Result<()> {
    db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; BEGIN;
        CREATE TABLE source(stamp TEXT NOT NULL, invalid_rings INTEGER NOT NULL); CREATE TABLE bad(id INTEGER PRIMARY KEY);
        CREATE TABLE rings(id INTEGER PRIMARY KEY,relation INTEGER NOT NULL);
        CREATE TABLE pieces(id INTEGER PRIMARY KEY,ring INTEGER,tier INTEGER,seq INTEGER,n INTEGER,offset INTEGER,stride INTEGER,anchor REAL,data BLOB);
        CREATE VIRTUAL TABLE ring_boxes USING rtree(id,west,east,south,north);
        CREATE VIRTUAL TABLE piece_boxes USING rtree(id,west,east,south,north,lod_min,lod_max);")?;
    let mut ring_row = db.prepare("INSERT INTO rings(relation) VALUES(?1)")?;
    let mut ring_box = db.prepare("INSERT INTO ring_boxes VALUES(?1,?2,?3,?4,?5)")?;
    let mut bad = db.prepare("INSERT OR IGNORE INTO bad VALUES(?1)")?;
    let mut piece = db.prepare(
        "INSERT INTO pieces(ring,tier,seq,n,offset,stride,anchor,data)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
    )?;
    let mut piece_box = db.prepare("INSERT INTO piece_boxes VALUES(?1,?2,?3,?4,?5,?6,?6)")?;
    let mut invalid_rings = 0u64;
    source::visit(file, |mut r| {
        let pts = &mut r.points;
        if r.level != level
            || pts.len() < 2
            || pts.iter().any(|p| {
                !p.lat.is_finite() || !p.lon.is_finite() || p.lat.abs() > 90. || p.lon.abs() > 180.
            })
        {
            invalid_rings += 1;
            bad.execute([r.relation])?;
            return Ok(());
        }
        if !r.polygon || pts.len() < 4 || pts.first() != pts.last() {
            bad.execute([r.relation])?;
        }
        let anchor = pts[0].lon;
        unwrap(pts, anchor);
        let ring = ring_row.insert([r.relation])?;
        let [w, s, e, n] = box_coords(pts);
        ring_box.execute(params![ring, w, e, s, n])?;
        let weights = importance(pts);
        for tier in 0..=FULL {
            let reduced;
            let line = if tier == FULL {
                &pts[..]
            } else {
                reduced = simplify(pts, &weights, tier);
                &reduced
            };
            for start in (0..line.len() - 1).step_by(256) {
                let chunk = &line[start..line.len().min(start + 257)];
                let data = (tier != FULL).then(|| {
                    let mut bytes = Vec::with_capacity(chunk.len() * 8);
                    for p in chunk {
                        bytes.extend(p.lon.to_le_bytes());
                        bytes.extend(p.lat.to_le_bytes());
                    }
                    bytes
                });
                let id = piece.insert(params![
                    ring,
                    tier,
                    start,
                    chunk.len(),
                    r.offset + (start * r.stride) as u64,
                    r.stride,
                    chunk[0].lon,
                    data
                ])?;
                let [w, s, e, n] = box_coords(chunk);
                piece_box.execute(params![id, w, e, s, n, tier])?;
            }
        }
        Ok(())
    })?;
    db.execute_batch("CREATE INDEX ring_pieces ON pieces(ring,tier,seq);")?;
    db.execute(
        "INSERT INTO source VALUES(?1,?2)",
        params![identity, invalid_rings],
    )?;
    db.execute_batch("COMMIT")?;
    Ok(())
}
