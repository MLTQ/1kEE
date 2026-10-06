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
