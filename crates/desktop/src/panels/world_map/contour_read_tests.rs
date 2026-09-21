//! Equivalence and opt-in measurements for the streaming contour reader.
use super::*;

fn request(path: &Path, tile: (i32, i32, i32)) -> (CacheKey, srtm_focus_cache::FocusContourAsset) {
    (
        CacheKey {
            path: path.to_owned(),
            zoom_bucket: tile.0,
            lat_bucket: tile.1,
            lon_bucket: tile.2,
        },
        srtm_focus_cache::FocusContourAsset {
            path: path.to_owned(),
            zoom_bucket: tile.0,
            lat_bucket: tile.1,
            lon_bucket: tile.2,
            simplify_step: 2,
        },
    )
}

// Previous query/decode path, retained only to check exact ordering and measure
// the complete read+decode operation against real, read-only operator data.
fn baseline(path: &Path, tile: (i32, i32, i32), budget: usize) -> Vec<ContourPath> {
    let connection = srtm_focus_cache::db::open_cache_db_read_only(path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT geom, elevation_m FROM contour_tiles
         WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3
         ORDER BY ABS(elevation_m), fid",
        )
        .unwrap();
    let mut rows = statement.query(params![tile.0, tile.1, tile.2]).unwrap();
    let mut contours = Vec::new();
    while let Some(row) = rows.next().unwrap() {
        let geometry: Vec<u8> = row.get(0).unwrap();
        let elevation_m = row.get(1).unwrap();
        for line in parse_gpkg_lines(&geometry) {
            if line.len() >= 2 {
                contours.push(ContourPath {
                    elevation_m,
                    points: simplify_line(line, 2),
                });
            }
        }
    }
    if contours.len() > budget {
        contours.sort_unstable_by(|a, b| b.points.len().cmp(&a.points.len()));
        contours.truncate(budget.max(1));
        contours.sort_by(|a, b| a.elevation_m.abs().total_cmp(&b.elevation_m.abs()));
    }
    contours
}

fn assert_same(actual: &[ContourPath], expected: &[ContourPath]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.elevation_m, expected.elevation_m);
        assert_eq!(actual.points, expected.points);
    }
}

#[test]
fn streamed_reader_preserves_elevation_fid_part_and_budget_order() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "1kee-read-order-{}-{nonce}.sqlite",
        std::process::id()
    ));
    let connection = srtm_focus_cache::db::open_cache_db(&path).unwrap();
    for (fid, elevation) in [(9, -5.0), (3, 5.0), (8, -0.5), (1, 20.0), (2, -20.0)] {
        let mut blob = b"GP\0\0\0\0\0\0".to_vec();
        blob.push(1);
        blob.extend_from_slice(&5u32.to_le_bytes()); // MultiLineString
        blob.extend_from_slice(&2u32.to_le_bytes());
        for part in 0..2 {
            blob.push(1);
            blob.extend_from_slice(&2u32.to_le_bytes());
            blob.extend_from_slice(&(fid as u32).to_le_bytes());
            for i in 0..fid {
                blob.extend_from_slice(&(fid as f64 + i as f64 / 100.0).to_le_bytes());
                blob.extend_from_slice(&(part as f64).to_le_bytes());
            }
        }
        connection
            .execute(
                "INSERT INTO contour_tiles VALUES (10,0,0,?1,?2,?3)",
                params![fid, elevation, blob],
            )
            .unwrap();
    }
    drop(connection);
    for budget in [0, 1, 3, 100] {
        let expected = baseline(&path, (10, 0, 0), budget);
        let result =
            query_local_contours_batch(&path, &[request(&path, (10, 0, 0))], budget).unwrap();
        assert_same(&result[0].1, &expected);
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore = "set ONEKEE_CONTOUR_BENCH_DB to a real contour cache; reads only"]
fn benchmark_dense_cached_tile() {
    let path =
        PathBuf::from(std::env::var_os("ONEKEE_CONTOUR_BENCH_DB").expect("benchmark cache path"));
    let connection = srtm_focus_cache::db::open_cache_db_read_only(&path).unwrap();
    let tile = connection
        .query_row(
            "SELECT zoom_bucket,lat_bucket,lon_bucket FROM contour_tile_manifest
         WHERE zoom_bucket=10 AND contour_count>5000 LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("dense 3DEP tile");
    drop(connection);
    // One warmup then alternate baseline/streamed. No timing assertion: the
    // filesystem cache and other apps vary. Geometry equality is mandatory.
    drop(baseline(&path, tile, usize::MAX));
    for _ in 0..3 {
        let start = Instant::now();
        let expected = baseline(&path, tile, usize::MAX);
        let before = start.elapsed();
        let start = Instant::now();
        let actual =
            query_local_contours_batch(&path, &[request(&path, tile)], usize::MAX).unwrap();
        let after = start.elapsed();
        assert_same(&actual[0].1, &expected);
        eprintln!(
            "tile {tile:?}: {} contours; baseline {before:?}, streamed {after:?}",
            expected.len()
        );
    }
}
