use super::*;

#[test]
#[ignore = "requires ONEKEE_SRTM_ROOT and local GDAL; source is read-only, output is temporary"]
fn native_yemen_build_keeps_source_resolution_at_globe_scale() {
    let source = PathBuf::from(std::env::var_os("ONEKEE_SRTM_ROOT").unwrap());
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("1kee-native-build-{nonce}"));
    fs::create_dir_all(&root).unwrap();
    let path = root.join(tile_archive::contour_grid::SRTM_NATIVE_DB_NAME);
    let spec = super::super::zoom::spec_for_zoom(0.5);
    let tile = TileKey {
        zoom_bucket: 0,
        lat_bucket: 9,
        lon_bucket: 27,
    };
    let grid = tile_archive::contour_grid::CoreTile::srtm(spec.half_extent_deg, 9, 27);
    let bounds = GeoBounds {
        min_lon: grid.source.min_lon as f32,
        max_lon: grid.source.max_lon as f32,
        min_lat: grid.source.min_lat as f32,
        max_lat: grid.source.max_lat as f32,
    };
    assert!(grid.raster_size > 5800);
    let start = std::time::Instant::now();
    build_focus_contours(&source, &root, &path, tile, bounds, spec).expect("native build");
    let db = super::super::db::open_cache_db_read_only(&path).unwrap();
    assert!(tile_archive::native_contours::is_native(&db).unwrap());
    let mut query = db
        .prepare("SELECT elevation_m,geom FROM contour_tiles")
        .unwrap();
    let mut rows = query.query([]).unwrap();
    let mut output = Vec::new();
    let mut lengths = Vec::new();
    while let Some(row) = rows.next().unwrap() {
        let elevation: f64 = row.get(0).unwrap();
        assert!((elevation / f64::from(spec.interval_m)).fract().abs() < 1e-6);
        let blob: Vec<u8> = row.get(1).unwrap();
        for points in tile_archive::gpkg::parse_gpkg_lines(&blob, |lon, lat| [lon, lat]) {
            for p in &points {
                assert!(p[0] >= grid.core.min_lon as f32 && p[0] <= grid.core.max_lon as f32);
                assert!(p[1] >= grid.core.min_lat as f32 && p[1] <= grid.core.max_lat as f32);
            }
            lengths.extend(
                points
                    .windows(2)
                    .map(|p| (p[0][0] - p[1][0]).abs().max((p[0][1] - p[1][1]).abs())),
            );
            output.push(serde_json::json!({"elevation": elevation, "points": points}));
        }
    }
    assert!(lengths.len() > 10_000);
    lengths.sort_by(f32::total_cmp);
    assert!(lengths[lengths.len() / 2] <= 1.0 / 3600.0 + 1e-5);
    eprintln!(
        "Native Yemen: {}px raster, {} paths, {} segments, median step {:.7} degrees, {:.2}s",
        grid.raster_size,
        output.len(),
        lengths.len(),
        lengths[lengths.len() / 2],
        start.elapsed().as_secs_f64()
    );
    if let Some(path) = std::env::var_os("ONEKEE_NATIVE_OUTPUT") {
        fs::write(path, serde_json::to_vec(&output).unwrap()).unwrap();
    }
    drop(rows);
    drop(query);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires SRTM, GDAL and GPU; benchmarks temporary caches only"]
fn gpu_srtm_end_to_end_benchmark() {
    assert!(
        !one_thousand_electric_eye_cache_builder::gpu_terrain::enabled(),
        "Run with ONEKEE_TERRAIN_GPU unset for a GDAL baseline"
    );
    let source = PathBuf::from(std::env::var_os("ONEKEE_SRTM_ROOT").unwrap());
    let root = tempfile::tempdir().unwrap();
    for (zoom, lat, lon, scale) in [(4, 126, 143, 5.0), (0, 9, 27, 0.5), (4, -98, -235, 5.0)] {
        let tile = TileKey {
            zoom_bucket: zoom,
            lat_bucket: lat,
            lon_bucket: lon,
        };
        let spec = super::super::zoom::spec_for_zoom(scale);
        assert_eq!(spec.zoom_bucket, zoom);
        let core = tile_archive::contour_grid::CoreTile::srtm(spec.half_extent_deg, lat, lon);
        let bounds = GeoBounds {
            min_lon: core.source.min_lon as f32,
            max_lon: core.source.max_lon as f32,
            min_lat: core.source.min_lat as f32,
            max_lat: core.source.max_lat as f32,
        };
        for round in 0..2 {
            let mut lengths = Vec::new();
            // Alternate order to expose source/cache warming rather than call it GPU speedup.
            for gpu in if round == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let dir = root
                    .path()
                    .join(format!("{zoom}-{lat}-{lon}-{round}-{gpu}"));
                fs::create_dir_all(&dir).unwrap();
                let path = dir.join(tile_archive::contour_grid::SRTM_NATIVE_DB_NAME);
                let start = Instant::now();
                if gpu {
                    let generated = one_thousand_electric_eye_cache_builder::gpu_terrain::build(
                        &source, zoom, lat, lon,
                    )
                    .expect("GPU generation, no silent fallback");
                    let mut db = super::super::db::open_cache_db(&path).unwrap();
                    generated.write(&mut db).unwrap();
                } else {
                    build_focus_contours(&source, &dir, &path, tile, bounds, spec).unwrap();
                }
                let elapsed = start.elapsed().as_secs_f64() * 1000.;
                let db = super::super::db::open_cache_db_read_only(&path).unwrap();
                let count: i64 = db
                    .query_row("SELECT contour_count FROM contour_tile_manifest", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                let rows: i64 = db
                    .query_row("SELECT COUNT(*) FROM contour_tiles", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, rows);
                assert!(count > 0);
                let mut query = db.prepare("SELECT geom FROM contour_tiles").unwrap();
                let mut segments = 0;
                let mut length = 0.0f64;
                let mut nonzero = 0;
                for blob in query.query_map([], |r| r.get::<_, Vec<u8>>(0)).unwrap() {
                    let lines =
                        tile_archive::gpkg::try_parse_gpkg_lines(&blob.unwrap(), |x, y| [x, y])
                            .unwrap();
                    for line in lines {
                        for p in &line {
                            assert!(
                                p[0] >= core.core.min_lon as f32
                                    && p[0] <= core.core.max_lon as f32
                                    && p[1] >= core.core.min_lat as f32
                                    && p[1] <= core.core.max_lat as f32
                            );
                        }
                        segments += line.len().saturating_sub(1);
                        for pair in line.windows(2) {
                            let d = f64::from(pair[1][0] - pair[0][0])
                                .hypot(f64::from(pair[1][1] - pair[0][1]));
                            length += d;
                            if d > 0.0 {
                                nonzero += 1;
                            }
                        }
                    }
                }
                eprintln!(
                    "GPU_BENCH tile={zoom}/{lat}/{lon} round={round} gpu={gpu} ms={elapsed:.3} paths={count} segments={segments} nonzero={nonzero} length_degrees={length:.9}"
                );
                lengths.push(length);
            }
            assert!(
                (lengths[0] - lengths[1]).abs() / lengths[0] < 0.001,
                "GPU/GDAL contour length mismatch: {lengths:?}"
            );
        }
    }
}
