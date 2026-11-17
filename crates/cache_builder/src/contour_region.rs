//! Copy only declared Earth contour/coastline tiles. The package snapshots the
//! original geometry bytes, counts, timestamps and native/legacy identity.
use crate::contours;
use ring::digest::{Context, SHA256};
use rusqlite::{Connection, OpenFlags, params};
use std::{fs, path::Path};
use tile_archive::{atlas::validate_bounds, contour_grid::SRTM_NATIVE_DB_NAME, native_contours};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const LEGACY: &str = "srtm_focus_cache.sqlite";
const USAGE: &str = "contour-region pack CACHE NEW_PACKAGE WEST SOUTH EAST NORTH | verify PACKAGE | install PACKAGE TERRAIN_DIRECTORY";
const WHERE_KEY: &str = "zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3";

pub fn run(args: &[String]) -> Result<()> {
    match args {
        [op, cache, out, w, s, e, n] if op == "pack" => pack(
            Path::new(cache),
            Path::new(out),
            [w.parse()?, s.parse()?, e.parse()?, n.parse()?],
        ),
        [op, path] if op == "verify" => check(Path::new(path), None),
        [op, path, dir] if op == "install" => check(Path::new(path), Some(Path::new(dir))),
        _ => Err(USAGE.into()),
    }
}
fn read(path: &Path) -> Result<Connection> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    c.execute_batch("PRAGMA trusted_schema=OFF; BEGIN;")?;
    Ok(c)
}
fn pack(source: &Path, out: &Path, bounds: [f64; 4]) -> Result<()> {
    validate_bounds(bounds)?;
    let db = read(source)?;
    let native = native_contours::is_native(&db)?;
    let cache = if native { SRTM_NATIVE_DB_NAME } else { LEGACY };
    if source.file_name().and_then(|s| s.to_str()) != Some(cache) {
        return Err(
            "Expected the corresponding Earth native or hosted cache filename and quality tag"
                .into(),
        );
    }
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?.into_temp_path();
    let mut stage = contours::open_cache_db(&temp)?;
    let tx = stage.transaction()?;
    let (count, hash) = transfer(&db, Some(&tx), bounds, native)?;
    if count == 0 {
        return Err("No cached tile manifests intersect the region".into());
    }
    tx.execute_batch("CREATE TABLE region_receipt(version INTEGER, cache TEXT, bounds BLOB, manifests INTEGER, sha256 BLOB);")?;
    tx.execute(
        "INSERT INTO region_receipt VALUES(1,?1,?2,?3,?4)",
        params![
            cache,
            bounds
                .into_iter()
                .flat_map(f64::to_le_bytes)
                .collect::<Vec<_>>(),
            count,
            hash
        ],
    )?;
    tx.commit()?;
    stage.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get::<_, String>(0))?;
    drop(stage);
    check(&temp, None)?;
    fs::File::open(&temp)?.sync_all()?;
    temp.persist_noclobber(out)?;
    Ok(())
}
fn check(path: &Path, install: Option<&Path>) -> Result<()> {
    let src = read(path)?;
    let (cache, bounds, count, expected): (String, Vec<u8>, u64, Vec<u8>) = src.query_row(
        "SELECT cache,bounds,manifests,sha256 FROM region_receipt WHERE version=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let native = match cache.as_str() {
        SRTM_NATIVE_DB_NAME => true,
        LEGACY => false,
        _ => return Err("Unknown Earth cache type".into()),
    };
    let raw: [u8; 32] = bounds.try_into().map_err(|_| "Invalid regional bounds")?;
    let bounds =
        std::array::from_fn(|i| f64::from_le_bytes(raw[i * 8..i * 8 + 8].try_into().unwrap()));
    validate_bounds(bounds)?;
    let verify = |actual: (u64, Vec<u8>)| -> Result<()> {
        if count == 0 || actual.0 != count || actual.1 != expected {
            return Err("Regional payload checksum or manifest count mismatch".into());
        }
        Ok(())
    };
    if let Some(dir) = install {
        let target = dir.join(cache);
        if target.exists() && target.canonicalize()? == path.canonicalize()? {
            return Err("Package is the destination".into());
        }
        // Fail before opening the destination when the portable file is bad.
        verify(transfer(&src, None, bounds, native)?)?;
        fs::create_dir_all(dir)?;
        let mut dest = contours::open_cache_db(&target)?;
        let tx = dest.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if native {
            native_contours::initialize(&tx)?;
        } else if native_contours::is_native(&tx)? {
            return Err("Native cache cannot receive legacy tiles".into());
        }
        verify(transfer(&src, Some(&tx), bounds, native)?)?;
        tx.commit()?;
        println!("Installed {count} tile manifests in {}", target.display());
    } else {
        verify(transfer(&src, None, bounds, native)?)?;
        println!("Verified {count} tile manifests in {}", path.display());
    }
    Ok(())
}
fn transfer(
    src: &Connection,
    dst: Option<&Connection>,
    bounds: [f64; 4],
    native: bool,
) -> Result<(u64, Vec<u8>)> {
    let mut hash = Context::new(&SHA256);
    hash.update(if native { b"native" } else { b"legacy" });
    for v in bounds {
        hash.update(&v.to_le_bytes());
    }
    let [w, s, e, n] = bounds;
    let mut total = 0;
    let specs = contours::all_specs();
    let tiers = specs
        .iter()
        .map(|s| s.half_extent_deg)
        .chain([0.210, 0.067, 0.0305, 0.0148]);
    for (z, half) in tiers.enumerate().take(if native { 7 } else { 11 }) {
        let step = f64::from(half * 0.45);
        let bucket = |v: f64| (v / step).round() as i32;
        let east = e.min(180.);
        let wrapped = e > 180.;
        for (kind, count_col) in [("contour", "contour_count"), ("coastline", "line_count")] {
            let manifests = format!("{kind}_tile_manifest");
            let rows_table = format!("{kind}_tiles");
            let mut st=src.prepare(&format!("SELECT lat_bucket,lon_bucket,{count_col},built_at FROM {manifests}
                WHERE zoom_bucket=?1 AND lat_bucket BETWEEN ?2 AND ?3 AND (lon_bucket BETWEEN ?4 AND ?5 OR (?6 AND lon_bucket BETWEEN ?7 AND ?8)) ORDER BY lat_bucket,lon_bucket"))?;
            let mut rows = st.query(params![
                z,
                bucket(s),
                bucket(n),
                bucket(w),
                bucket(east),
                wrapped,
                bucket(-180.),
                bucket(e - 360.)
            ])?;
            while let Some(row) = rows.next()? {
                let y: i32 = row.get(0)?;
                let x: i32 = row.get(1)?;
                let count: i64 = row.get(2)?;
                let built: i64 = row.get(3)?;
                hash.update(kind.as_bytes());
                for v in [z as i64, y.into(), x.into(), count, built] {
                    hash.update(&v.to_le_bytes());
                }
                if let Some(dst) = dst {
                    dst.execute(
                        &format!("DELETE FROM {rows_table} WHERE {WHERE_KEY}"),
                        params![z, y, x],
                    )?;
                    dst.execute(&format!("INSERT OR REPLACE INTO {manifests}(zoom_bucket,lat_bucket,lon_bucket,{count_col},built_at) VALUES(?1,?2,?3,?4,?5)"),params![z,y,x,count,built])?;
                }
                let elevation = if kind == "contour" {
                    "elevation_m"
                } else {
                    "0.0"
                };
                let mut st = src.prepare_cached(&format!(
                    "SELECT fid,{elevation},geom FROM {rows_table} WHERE {WHERE_KEY} ORDER BY fid"
                ))?;
                let mut features = st.query(params![z, y, x])?;
                let mut seen = 0;
                while let Some(record) = features.next()? {
                    let fid: i64 = record.get(0)?;
                    let z_m: f64 = record.get(1)?;
                    let geom = record.get_ref(2)?.as_blob()?;
                    if !z_m.is_finite()
                        || tile_archive::gpkg::try_parse_gpkg_lines(geom, |_, _| ()).is_none()
                    {
                        return Err(format!("Invalid {kind} geometry {z}/{y}/{x} fid {fid}").into());
                    }
                    hash.update(&fid.to_le_bytes());
                    hash.update(&z_m.to_bits().to_le_bytes());
                    hash.update(&(geom.len() as u64).to_le_bytes());
                    hash.update(geom);
                    if let Some(dst) = dst {
                        if kind == "contour" {
                            dst.prepare_cached("INSERT INTO contour_tiles(zoom_bucket,lat_bucket,lon_bucket,fid,elevation_m,geom) VALUES(?1,?2,?3,?4,?5,?6)")?.execute(params![z,y,x,fid,z_m,geom])?;
                        } else {
                            dst.prepare_cached("INSERT INTO coastline_tiles(zoom_bucket,lat_bucket,lon_bucket,fid,geom) VALUES(?1,?2,?3,?4,?5)")?.execute(params![z,y,x,fid,geom])?;
                        }
                    }
                    seen += 1;
                }
                if seen != count {
                    return Err(
                        format!("Incomplete {kind} tile {z}/{y}/{x}: {seen} of {count}").into(),
                    );
                }
                total += 1;
            }
        }
    }
    Ok((total, hash.finish().as_ref().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installs_exact_bounds_and_keeps_unrelated_tiles_on_update_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join(LEGACY);
        let src = contours::open_cache_db(&source).unwrap();
        src.execute("INSERT INTO contour_tile_manifest(zoom_bucket,lat_bucket,lon_bucket,contour_count,built_at) VALUES(0,0,0,0,101)",[]).unwrap();
        let package = tmp.path().join("portable");
        pack(
            &source,
            &package,
            [-0.00010000000000000003, -0.01, 0.1, 0.1],
        )
        .unwrap();
        let dir = tmp.path().join("installed");
        fs::create_dir(&dir).unwrap();
        check(&package, Some(&dir)).unwrap();
        let db = contours::open_cache_db(&dir.join(LEGACY)).unwrap();
        db.execute("INSERT INTO contour_tile_manifest(zoom_bucket,lat_bucket,lon_bucket,contour_count,built_at) VALUES(0,15,18,0,201)",[]).unwrap();
        check(&package, Some(&dir)).unwrap();
        let bad = Connection::open(&package).unwrap();
        bad.execute("UPDATE region_receipt SET sha256=zeroblob(32)", [])
            .unwrap();
        assert!(check(&package, Some(&dir)).is_err());
        assert_eq!(
            db.query_row("SELECT SUM(built_at) FROM contour_tile_manifest", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            302
        );
    }
}
