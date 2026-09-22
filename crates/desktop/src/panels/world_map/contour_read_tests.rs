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
fn packed_contours_match_runtime_order_budget_and_source_updates() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "1kee-packed-contour-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("terrain")).unwrap();
    let path = root.join("terrain/srtm_focus_cache.sqlite");
    let connection = srtm_focus_cache::db::open_cache_db(&path).unwrap();
    let mut source_rows = Vec::new();
    for (fid, elevation) in [(1, 20.0), (2, -20.0), (3, 5.0), (4, -0.5)] {
        let mut blob = b"GP\0\0\0\0\0\0".to_vec();
        blob.push(1);
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(&5u32.to_le_bytes());
        for i in 0..5 {
            blob.extend_from_slice(&(i as f64).to_le_bytes());
            blob.extend_from_slice(&(fid as f64).to_le_bytes());
        }
        connection
            .execute(
                "INSERT INTO contour_tiles VALUES (10,0,0,?1,?2,?3)",
                params![fid, elevation, &blob],
            )
            .unwrap();
        source_rows.push((elevation as f32, blob));
    }
    connection.execute("INSERT INTO contour_tile_manifest(zoom_bucket,lat_bucket,lon_bucket,contour_count) VALUES(10,0,0,4)",[]).unwrap();
    drop(connection);
    let archive_path = root.join(tile_archive::FILE_NAME);
    let mut writer = tile_archive::Writer::create(&archive_path).unwrap();
    writer
        .put_batch(
            &[(
                tile_archive::Key {
                    body: 0,
                    layer: *b"CNTR",
                    grid: 2,
                    level: 10,
                    y: 0,
                    x: 0,
                },
                tile_archive::contours::encode(&source_rows).unwrap(),
            )],
            None,
        )
        .unwrap();
    writer
        .metadata(
            "contours:0",
            &tile_archive::contours::fingerprint(&path).unwrap(),
        )
        .unwrap();
    writer.finish().unwrap();
    assert!(
        crate::world_archive::ContourArchive::open(&path)
            .unwrap()
            .get(10, 0, 0)
            .is_some()
    );
    for budget in [0, 1, 3, 100] {
        let expected = baseline(&path, (10, 0, 0), budget);
        let actual =
            query_local_contours_batch(&path, &[request(&path, (10, 0, 0))], budget).unwrap();
        assert_same(&actual[0].1, &expected);
    }
    let connection = srtm_focus_cache::db::open_cache_db(&path).unwrap();
    connection
        .execute("UPDATE contour_tiles SET elevation_m=123 WHERE fid=1", [])
        .unwrap();
    assert!(crate::world_archive::ContourArchive::open(&path).is_none());
    drop(connection);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "real read/decode benchmark; ONEKEE_CONTOUR_BENCH_DB and ONEKEE_ARCHIVE_BENCH_DIR required"]
fn benchmark_packed_contour_tile() {
    let source_path =
        PathBuf::from(std::env::var_os("ONEKEE_CONTOUR_BENCH_DB").expect("source database"));
    let parent = PathBuf::from(
        std::env::var_os("ONEKEE_ARCHIVE_BENCH_DIR").expect("disposable output parent"),
    );
    let source = srtm_focus_cache::db::open_cache_db_read_only(&source_path).unwrap();
    let tile:(i32,i32,i32)=source.query_row("SELECT zoom_bucket,lat_bucket,lon_bucket FROM contour_tile_manifest ORDER BY contour_count DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    let mut stmt=source.prepare("SELECT fid,elevation_m,geom FROM contour_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3 ORDER BY fid").unwrap();
    let mut cursor = stmt.query(params![tile.0, tile.1, tile.2]).unwrap();
    let mut rows = Vec::new();
    let mut total_bytes = 0;
    while let Some(row) = cursor.next().unwrap() {
        let geometry = row.get_ref(2).unwrap().as_blob().unwrap();
        total_bytes += geometry.len() + 8;
        assert!(
            total_bytes < tile_archive::MAX_PAYLOAD,
            "choose a smaller benchmark source"
        );
        rows.push((
            row.get::<_, i64>(0).unwrap(),
            row.get::<_, f32>(1).unwrap(),
            geometry.to_vec(),
        ));
    }
    let root = parent.join(format!(
        "contour-read-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("terrain")).unwrap();
    let path = root.join("terrain/srtm_focus_cache.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    srtm_focus_cache::db::ensure_cache_schema_with_connection(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    for (fid, elevation, geometry) in &rows {
        tx.execute(
            "INSERT INTO contour_tiles VALUES(?1,?2,?3,?4,?5,?6)",
            params![tile.0, tile.1, tile.2, fid, elevation, geometry],
        )
        .unwrap();
    }
    tx.execute("INSERT INTO contour_tile_manifest(zoom_bucket,lat_bucket,lon_bucket,contour_count) VALUES(?1,?2,?3,?4)",params![tile.0,tile.1,tile.2,rows.len()]).unwrap();
    tx.commit().unwrap();
    drop(conn);
    let archive_path = root.join(tile_archive::FILE_NAME);
    let hidden = root.join("hidden.1ka");
    let mut writer = tile_archive::Writer::create(&archive_path).unwrap();
    let packed = tile_archive::contours::encode(
        &rows.into_iter().map(|(_, e, g)| (e, g)).collect::<Vec<_>>(),
    )
    .unwrap();
    writer
        .put_batch(
            &[(
                tile_archive::Key {
                    body: 0,
                    layer: *b"CNTR",
                    grid: 2,
                    level: tile.0,
                    y: tile.1,
                    x: tile.2,
                },
                packed,
            )],
            None,
        )
        .unwrap();
    writer
        .metadata(
            "contours:0",
            &tile_archive::contours::fingerprint(&path).unwrap(),
        )
        .unwrap();
    writer.finish().unwrap();
    assert!(crate::world_archive::ContourArchive::open(&path).is_some());
    let mut expected: Option<Vec<ContourPath>> = None;
    for archived in [false, true, true, false, false, true, true, false] {
        if !archived {
            std::fs::rename(&archive_path, &hidden).unwrap();
        }
        let start = Instant::now();
        let actual = query_local_contours_batch(&path, &[request(&path, tile)], 100_000).unwrap();
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        if !archived {
            std::fs::rename(&hidden, &archive_path).unwrap();
        }
        if let Some(expected) = &expected {
            assert_same(&actual[0].1, expected);
        } else {
            expected = Some(actual[0].1.clone());
        }
        eprintln!(
            "PACKED_CONTOUR archived={archived} read_decode_ms={ms:.3} contours={} geometry_bytes={total_bytes}",
            actual[0].1.len()
        );
    }
    std::fs::remove_dir_all(root).unwrap();
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
fn row_progress_uses_manifest_count_and_reaches_the_actual_decoded_count() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "1kee-row-progress-{}-{nonce}.sqlite",
        std::process::id()
    ));
    let connection = srtm_focus_cache::db::open_cache_db(&path).unwrap();
    // Empty geometry still counts as a decoded database row, not a drawn line.
    connection
        .execute_batch(
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<300)
         INSERT INTO contour_tiles SELECT 10,0,0,x,10.0,x'00' FROM n;
         INSERT INTO contour_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,contour_count)
         VALUES (10,0,0,300);",
        )
        .unwrap();
    drop(connection);
    let mut updates = Vec::new();
    let result = query_local_contours_with_progress(
        &path,
        &[request(&path, (10, 0, 0))],
        100,
        &mut |key, done, total| {
            assert_eq!(key.path, path);
            updates.push((done, total));
        },
    )
    .unwrap();
    assert_eq!(updates, vec![(128, 300), (256, 300), (300, 300)]);
    assert!(result[0].1.is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn streamed_tiles_publish_before_batch_end_and_respect_resets() {
    let requests = [
        request(Path::new("test.sqlite"), (10, 0, 0)),
        request(Path::new("test.sqlite"), (10, 0, 1)),
    ];
    let assets: Vec<_> = requests.iter().map(|(_, asset)| asset.clone()).collect();
    let mut state = LocalRegionCache::default();
    let (epoch, batch) = begin_local_read(&mut state, &assets, 0, 0).unwrap();
    let cache = Mutex::new(state);
    let contours = vec![ContourPath {
        elevation_m: 10.0,
        points: vec![
            GeoPoint { lat: 0.0, lon: 0.0 },
            GeoPoint { lat: 1.0, lon: 1.0 },
        ],
    }];
    assert!(reader::publish_tile(
        &cache,
        epoch,
        requests[0].0.clone(),
        contours
    ));
    {
        let mut state = cache.lock().unwrap();
        assert!(state.entries.contains_key(&requests[0].0));
        assert!(!state.in_flight.contains(&requests[0].0));
        assert!(state.in_flight.contains(&requests[1].0));
        assert_eq!(state.load_in_flight, Some(epoch));
        assert!(
            state.published_tiles.is_empty(),
            "decode is not render completion"
        );
        assert!(begin_local_read(&mut state, &assets, 0, 0).is_none());
        state.reset_all();
    }
    assert!(!reader::publish_tile(
        &cache,
        epoch,
        requests[1].0.clone(),
        Vec::new()
    ));
    finish_local_read(&cache, epoch, &batch, Some(Vec::new()));
    let mut state = cache.lock().unwrap();
    assert!(state.entries.is_empty());
    assert!(state.load_in_flight.is_none());
    let (new_epoch, _) = begin_local_read(&mut state, &assets, 0, 0).unwrap();
    drop(state);
    assert_ne!(epoch, new_epoch);
    assert!(!reader::publish_tile(
        &cache,
        epoch,
        requests[0].0.clone(),
        Vec::new()
    ));
    finish_local_read(&cache, epoch, &batch, None);
    assert_eq!(cache.lock().unwrap().load_in_flight, Some(new_epoch));
}

#[test]
fn streaming_reader_can_cancel_between_tiles() {
    let path =
        std::env::temp_dir().join(format!("1kee-stream-cancel-{}.sqlite", std::process::id()));
    let connection = srtm_focus_cache::db::open_cache_db(&path).unwrap();
    connection
        .execute_batch(
            "DELETE FROM contour_tiles;
         INSERT INTO contour_tiles VALUES (10,0,0,1,10.0,x'00'),(10,0,1,1,10.0,x'00');",
        )
        .unwrap();
    drop(connection);
    let requests = [request(&path, (10, 0, 0)), request(&path, (10, 0, 1))];
    let mut tiles = Vec::new();
    stream_local_contours(&path, &requests, 100, &mut |_, _, _| {}, &mut |key, _| {
        tiles.push(key);
        false
    })
    .unwrap();
    assert!(tiles == vec![requests[0].0.clone()]);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn manifest_publication_preserves_revision_observed_before_selection() {
    let key = LocalManifestKey {
        root: None,
        center_lat_bucket: 0,
        center_lon_bucket: 0,
        zoom_bucket: 10,
        prefetch_radius: 2,
        build_radius: 2,
    };
    let cache = Mutex::new(LocalRegionCache {
        manifest_requested_key: Some(key.clone()),
        manifest_in_flight: Some(key.clone()),
        ..Default::default()
    });
    // Queue capacity can change while SQLite selection is in progress. The
    // snapshot must retain the worker's revision, not stamp the latest global
    // revision over it and suppress the next scheduling pass.
    finish_local_manifest(&cache, &key, Some(Vec::new()), BuildSnapshot::new(), 41);
    let state = cache.lock().unwrap();
    assert_eq!(
        state.manifest_snapshot.as_ref().unwrap().manifest_revision,
        41
    );
    assert!(state.manifest_in_flight.is_none());
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

#[test]
#[ignore = "set ONEKEE_CONTOUR_BENCH_DB; reads eight dense tiles without modifying the cache"]
fn benchmark_cached_reader_concurrency() {
    use std::hash::{Hash, Hasher};
    let path = PathBuf::from(std::env::var_os("ONEKEE_CONTOUR_BENCH_DB").expect("cache path"));
    let connection = srtm_focus_cache::db::open_cache_db_read_only(&path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT zoom_bucket,lat_bucket,lon_bucket FROM contour_tile_manifest
         WHERE zoom_bucket=10 AND contour_count>5000 ORDER BY lat_bucket,lon_bucket LIMIT 8",
        )
        .unwrap();
    let requests: Vec<_> = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| request(&path, r.unwrap()))
        .collect();
    assert_eq!(requests.len(), 8, "need eight dense tiles");
    let mut reference = None;
    for workers in [1usize, 2, 4, 2, 1] {
        let start = Instant::now();
        let fingerprints = std::thread::scope(|scope| {
            let handles: Vec<_> = requests
                .chunks(requests.len().div_ceil(workers))
                .map(|chunk| {
                    let path = &path;
                    scope.spawn(move || {
                        query_local_contours_batch(path, chunk, 1500)
                            .unwrap()
                            .into_iter()
                            .map(|(key, contours)| {
                                let mut hash = std::collections::hash_map::DefaultHasher::new();
                                key.hash(&mut hash);
                                for contour in &contours {
                                    contour.elevation_m.to_bits().hash(&mut hash);
                                    contour.points.len().hash(&mut hash);
                                    for point in &contour.points {
                                        point.lat.to_bits().hash(&mut hash);
                                        point.lon.to_bits().hash(&mut hash);
                                    }
                                }
                                hash.finish()
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        if let Some(expected) = &reference {
            assert_eq!(&fingerprints, expected);
        }
        reference = Some(fingerprints);
        eprintln!(
            "READERS workers={workers} tiles=8 wall_ms={:.1}",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}
