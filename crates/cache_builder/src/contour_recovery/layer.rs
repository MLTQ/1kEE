//! A manifest is authoritative for counts, even when an index or a range of pages is damaged.
use anyhow::{Result, anyhow, bail, ensure};
use ring::digest::{Context, SHA256};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;

pub(super) type Key = [i32; 3];
pub(super) type Hash = [u8; 32];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Layer {
    Contour,
    Coast,
}

pub(super) const LAYERS: [Layer; 2] = [Layer::Contour, Layer::Coast];

impl Layer {
    pub fn table(self) -> &'static str {
        match self {
            Self::Contour => "contour_tiles",
            Self::Coast => "coastline_tiles",
        }
    }
    pub fn manifest(self) -> &'static str {
        match self {
            Self::Contour => "contour_tile_manifest",
            Self::Coast => "coastline_tile_manifest",
        }
    }
    pub fn count(self) -> &'static str {
        match self {
            Self::Contour => "contour_count",
            Self::Coast => "line_count",
        }
    }
    fn elevation(self) -> &'static str {
        if self == Self::Contour {
            "elevation_m"
        } else {
            "0.0"
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Tile {
    pub key: Key,
    pub count: i64,
    pub built: i64,
    first: Option<i64>,
}

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, Tile)> {
    Ok((
        row.get(0)?,
        Tile {
            key: [row.get(1)?, row.get(2)?, row.get(3)?],
            count: row.get(4)?,
            built: row.get(5)?,
            first: None,
        },
    ))
}

pub(super) fn corrupt(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
    )
}

pub(super) fn inventory(db: &Connection, layer: Layer) -> Result<(Vec<Tile>, bool)> {
    let sql = format!(
        "SELECT rowid,zoom_bucket,lat_bucket,lon_bucket,{},built_at FROM {} NOT INDEXED",
        layer.count(),
        layer.manifest()
    );
    let regular = db
        .prepare(&(sql.clone() + " ORDER BY rowid"))?
        .query_map([], from_row)?
        .collect::<rusqlite::Result<Vec<_>>>();
    let (mut rows, mut complete) = match regular {
        Ok(rows) => (rows, true),
        Err(e) if corrupt(&e) => {
            let max: i64 = db.query_row(
                &format!("SELECT COALESCE(MAX(rowid),0) FROM {}", layer.manifest()),
                [],
                |r| r.get(0),
            )?;
            let mut rows = Vec::new();
            let mut stmt = db.prepare(&(sql + " WHERE rowid=?"))?;
            for id in 1..=max {
                match stmt.query_row([id], from_row).optional() {
                    Ok(Some(row)) => rows.push(row),
                    Ok(None) => {}
                    Err(e) if corrupt(&e) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            (rows, false)
        }
        Err(e) => return Err(e.into()),
    };
    // Native builders append a tile's geometry in one transaction. Use this fast
    // path only when every row proves its identity. A changed layout falls back
    // to lookup by key. Recover a final committed WAL run independently of gaps.
    rows.retain(|(_, tile)| {
        let valid = tile.count >= 0 && (0..=6).contains(&tile.key[0]);
        complete &= valid;
        valid
    });
    let mut next = Some(1i64);
    let mut previous = 0i64;
    for (id, tile) in &mut rows {
        if previous.checked_add(1i64) != Some(*id) {
            next = None;
        }
        tile.first = next;
        next = next.and_then(|n| n.checked_add(tile.count));
        previous = *id;
    }
    if rows.last().is_some_and(|(_, t)| t.first.is_none()) {
        let mut last: Option<i64> = db
            .query_row(
                &format!("SELECT MAX(rowid) FROM {}", layer.table()),
                [],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        let mut following = rows.last().map(|(id, _)| *id);
        for (id, tile) in rows.iter_mut().rev() {
            if Some(*id) != following || tile.first.is_some() {
                break;
            }
            tile.first = last.and_then(|v| v.checked_sub(tile.count)?.checked_add(1));
            last = tile.first.and_then(|v| v.checked_sub(1));
            following = id.checked_sub(1);
        }
    }
    Ok((rows.into_iter().map(|(_, t)| t).collect(), complete))
}

fn select(layer: Layer, range: bool) -> String {
    let filter = if range {
        "NOT INDEXED WHERE rowid BETWEEN ?1 AND ?2 ORDER BY rowid"
    } else {
        "WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3 ORDER BY fid"
    };
    format!(
        "SELECT zoom_bucket,lat_bucket,lon_bucket,fid,{},geom FROM {} {filter}",
        layer.elevation(),
        layer.table()
    )
}

pub(super) fn transfer(
    src: &Connection,
    dst: Option<&Connection>,
    layer: Layer,
    tile: &Tile,
    range: bool,
) -> Result<Hash> {
    let mut hashes = BTreeMap::new();
    // Empty manifests are useful coverage. For a normal indexed read, check
    // that no unexpected geometry hides behind a zero count as well.
    if tile.count != 0 || !range {
        let mut query = src.prepare_cached(&select(layer, range))?;
        let mut rows = if range {
            let first = tile
                .first
                .ok_or_else(|| anyhow!("no contiguous row range"))?;
            query.query(params![
                first,
                first
                    .checked_add(tile.count - 1)
                    .ok_or_else(|| anyhow!("row range overflow"))?
            ])?
        } else {
            query.query(tile.key)?
        };
        let insert = if layer == Layer::Contour {
            "INSERT INTO contour_tiles VALUES (?1,?2,?3,?4,?5,?6)"
        } else {
            "INSERT INTO coastline_tiles VALUES (?1,?2,?3,?4,?6)"
        };
        let mut writer = dst.map(|db| db.prepare_cached(insert)).transpose()?;
        while let Some(row) = rows.next()? {
            let key: Key = [row.get(0)?, row.get(1)?, row.get(2)?];
            ensure!(key == tile.key, "geometry belongs to a different tile");
            let fid: i64 = row.get(3)?;
            let elevation: f64 = row.get(4)?;
            let blob = row.get_ref(5)?.as_blob()?;
            let mut finite = true;
            let lines = tile_archive::gpkg::try_parse_gpkg_lines(blob, |x, y| {
                finite &= x.is_finite() && y.is_finite();
            });
            ensure!(
                elevation.is_finite() && finite && lines.is_some(),
                "invalid contour geometry"
            );
            let mut h = Context::new(&SHA256);
            h.update(&fid.to_le_bytes());
            h.update(&elevation.to_bits().to_le_bytes());
            h.update(blob);
            ensure!(
                hashes.insert(fid, h.finish()).is_none(),
                "duplicate feature ID"
            );
            if let Some(stmt) = writer.as_mut() {
                stmt.execute(params![key[0], key[1], key[2], fid, elevation, blob])?;
            }
        }
    }
    ensure!(
        hashes.len() as i64 == tile.count,
        "expected {} rows, read {}",
        tile.count,
        hashes.len()
    );
    let mut hash = Context::new(&SHA256);
    for h in hashes.values() {
        hash.update(h.as_ref());
    }
    if let Some(dst) = dst {
        dst.prepare_cached(&format!(
            "INSERT INTO {} VALUES (?1,?2,?3,?4,?5)",
            layer.manifest()
        ))?
        .execute(params![
            tile.key[0],
            tile.key[1],
            tile.key[2],
            tile.count,
            tile.built
        ])?;
    }
    Ok(hash.finish().as_ref().try_into()?)
}

pub(super) fn copy_tile(
    src: &Connection,
    dst: &mut Connection,
    layer: Layer,
    tile: &Tile,
    replace: bool,
    manifest_intact: bool,
) -> Result<Hash> {
    if manifest_intact {
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE zoom_bucket=? AND lat_bucket=? AND lon_bucket=?",
            layer.table()
        );
        match src
            .prepare_cached(&sql)?
            .query_row(tile.key, |r| r.get::<_, i64>(0))
        {
            Ok(n) => ensure!(n == tile.count, "manifest disagrees with feature count"),
            Err(e) if corrupt(&e) => {}
            Err(e) => return Err(e.into()),
        }
    }
    for range in [true, false] {
        let tx = dst.transaction()?;
        if replace {
            for table in [layer.table(), layer.manifest()] {
                tx.execute(&format!("DELETE FROM {table} WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3"),tile.key)?;
            }
        }
        match transfer(src, Some(&tx), layer, tile, range) {
            Ok(hash) => {
                tx.commit()?;
                return Ok(hash);
            }
            Err(e) if range => {
                tx.rollback()?;
                if !super::keep_error(&e) {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
    }
    bail!("tile read failed")
}
