use super::*;

#[test]
#[ignore = "downloads one small Japan GSI window and runs GDAL contouring"]
fn live_japan_contours_commit_to_the_existing_cache() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("1kee-japan-build-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("srtm_focus_cache.sqlite");
    let spec = super::super::zoom::spec_for_zoom(60.0);
    let step = spec.half_extent_deg * 0.45;
    let tile = TileKey {
        zoom_bucket: spec.zoom_bucket,
        lat_bucket: (35.681_f32 / step).round() as i32,
        lon_bucket: (139.767_f32 / step).round() as i32,
    };
    let core = tile_archive::contour_grid::CoreTile::new(
        spec.half_extent_deg,
        spec.raster_size,
        tile.lat_bucket,
        tile.lon_bucket,
    );
    let bounds = GeoBounds {
        min_lon: core.source.min_lon as f32,
        max_lon: core.source.max_lon as f32,
        min_lat: core.source.min_lat as f32,
        max_lat: core.source.max_lat as f32,
    };
    let download = super::super::work_slots::remote_downloads()
        .try_acquire()
        .unwrap();
    assert!(build_threedep_contours(&root, &db_path, tile, bounds, spec, download).is_some());
    let connection = super::super::db::open_cache_db_read_only(&db_path).unwrap();
    assert!(super::super::db::tile_exists(&connection, tile).unwrap());
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM contour_tiles", [], |row| row.get(0))
        .unwrap();
    assert!(count > 0);
    eprintln!("Japan deep-zoom tile stored {count} contour paths");
    drop(connection);
    assert!(
        std::fs::read_dir(&root).unwrap().all(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with("srtm_focus_cache.sqlite")
                || (name == super::super::TEMP_DIR_NAME
                    && std::fs::read_dir(entry.path()).unwrap().next().is_none())
        }),
        "only the cache database and its empty shared staging directory may remain"
    );
    std::fs::remove_dir_all(root).unwrap();
}
