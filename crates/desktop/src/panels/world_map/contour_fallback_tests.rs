use super::*;
use crate::panels::world_map::local_contour_pass::LocalTileId;

fn frame(zoom: f32, radius: i32, with_lines: bool) -> LocalContourLoad {
    let bucket = srtm_focus_cache::zoom_bucket_for_zoom(zoom);
    let lines = Arc::new(if with_lines {
        vec![ContourPath {
            elevation_m: 50.0,
            points: vec![
                GeoPoint { lat: 0.0, lon: 0.0 },
                GeoPoint {
                    lat: 0.001,
                    lon: 0.001,
                },
            ],
        }]
    } else {
        vec![]
    });
    let mut tiles = Vec::new();
    for lat in -radius..=radius {
        for lon in -radius..=radius {
            tiles.push(LocalTileGeometry {
                id: LocalTileId {
                    zoom_bucket: bucket,
                    lat_bucket: lat,
                    lon_bucket: lon,
                },
                contours: Arc::clone(&lines),
            });
        }
    }
    LocalContourLoad {
        source_zoom: zoom,
        build_radius: radius,
        contours: Some(lines),
        ready_buckets: tiles
            .iter()
            .map(|t| (t.id.lat_bucket, t.id.lon_bucket))
            .collect(),
        loading_progress: HashMap::new(),
        status: srtm_focus_cache::FocusContourRegionStatus {
            ready_assets: tiles.len(),
            pending_assets: 0,
            total_assets: tiles.len(),
        },
        tiles,
    }
}

#[test]
fn missing_fine_coverage_keeps_base_geometry_and_its_progress_grid() {
    let mut fine = frame(60.0, 3, false);
    // Known unavailable cells can all be marked ready without any actual tile.
    fine.tiles.clear();
    assert!(!covers_view(&fine, GeoPoint { lat: 0.0, lon: 0.0 }, 60.0));
    let mut base = frame(BASE_ZOOM, 1, true);
    base.loading_progress.insert((0, 0), 0.87);
    let expected = Arc::clone(base.contours.as_ref().unwrap());
    let chosen = choose(fine, base);
    assert_eq!(chosen.source_zoom, BASE_ZOOM);
    assert_eq!(chosen.build_radius, 1);
    assert_eq!(chosen.loading_progress[&(0, 0)], 0.87);
    assert!(Arc::ptr_eq(chosen.contours.as_ref().unwrap(), &expected));
    assert!(chosen.tiles.iter().all(|t| t.id.zoom_bucket == BASE_BUCKET));
}

#[test]
fn fine_takeover_requires_visible_tiles_but_not_the_prefetch_ring() {
    let center = GeoPoint { lat: 0.0, lon: 0.0 };
    // At maximum zoom, the visible window uses radius two; radius three is
    // prefetch only. Offline decoded empty tiles are valid coverage as well.
    let mut fine = frame(60.0, 2, false);
    assert!(covers_view(&fine, center, 60.0));
    fine.ready_buckets.remove(&(2, 2));
    assert!(!covers_view(&fine, center, 60.0)); // wait for merge publication
    fine.ready_buckets.insert((2, 2));
    fine.tiles
        .retain(|t| (t.id.lat_bucket, t.id.lon_bucket) != (2, 2));
    assert!(!covers_view(&fine, center, 60.0));
    fine.tiles.push(LocalTileGeometry {
        id: LocalTileId {
            zoom_bucket: BASE_BUCKET,
            lat_bucket: 2,
            lon_bucket: 2,
        },
        contours: Arc::new(vec![]),
    });
    assert!(!covers_view(&fine, center, 60.0)); // wrong tier cannot fill the hole
}

#[test]
fn fallback_envelope_covers_every_deep_zoom_at_cell_edges() {
    let spec = srtm_focus_cache::zoom::spec_for_zoom(BASE_ZOOM);
    assert_eq!(spec.zoom_bucket, BASE_BUCKET);
    for quarter_zoom in 52..=240 {
        let view_zoom = quarter_zoom as f32 / 4.0;
        let radius = base_radius(view_zoom);
        assert!(
            srtm_focus_cache::region_coverage_half_extent_deg(&spec, radius)
                >= visible_half_extent(view_zoom)
        );
    }
    assert_eq!(base_radius(60.0), 1); // nine base tiles, not the base camera's 13×13
}

#[test]
fn panning_outside_loaded_fine_tiles_reactivates_base() {
    let fine = frame(60.0, 3, true);
    assert!(covers_view(&fine, GeoPoint { lat: 0.0, lon: 0.0 }, 60.0));
    assert!(!covers_view(
        &fine,
        GeoPoint {
            lat: 0.02,
            lon: 0.02
        },
        60.0
    ));
}

#[test]
fn fine_arrivals_remain_visible_if_no_base_source_exists() {
    let fine = frame(60.0, 0, true);
    let chosen = choose(fine, frame(BASE_ZOOM, 1, false));
    assert_eq!(chosen.source_zoom, 60.0);
}

/// Uses the public asynchronous loader and a temporary SQLite cache, never
/// the user's terrain. Isolated because the public loader owns global caches.
#[test]
#[ignore = "run separately: exercises global Earth loader caches"]
fn cold_deep_zoom_reads_base_then_upgrades_to_cached_fine_tiles() {
    let root = std::env::temp_dir().join(format!(
        "1kee-fallback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let derived = root.join("Derived");
    std::fs::create_dir_all(derived.join("terrain")).unwrap();
    let db = derived.join("terrain/srtm_focus_cache.sqlite");
    let connection = srtm_focus_cache::db::open_cache_db(&db).unwrap();
    let center = GeoPoint {
        lat: -33.86,
        lon: 151.21,
    }; // Sydney: outside every hosted provider, so this remains offline
    let insert = |zoom: f32, radius: i32| {
        let spec = srtm_focus_cache::zoom::spec_for_zoom(zoom);
        let step = spec.half_extent_deg * 0.45;
        let y = (center.lat / step).round() as i32;
        let x = (center.lon / step).round() as i32;
        for lat in y - radius..=y + radius {
            for lon in x - radius..=x + radius {
                let mut blob = b"GP\0\0\0\0\0\0".to_vec();
                blob.push(1);
                blob.extend_from_slice(&2u32.to_le_bytes());
                blob.extend_from_slice(&2u32.to_le_bytes());
                for offset in [-0.0001, 0.0001] {
                    blob.extend_from_slice(&(f64::from(lon as f32 * step) + offset).to_le_bytes());
                    blob.extend_from_slice(&f64::from(lat as f32 * step).to_le_bytes());
                }
                // More than the old 120-line limit; short paths must survive.
                for fid in 0..150 {
                    connection
                        .execute(
                            "INSERT INTO contour_tiles VALUES (?1,?2,?3,?4,?5,?6)",
                            params![spec.zoom_bucket, lat, lon, fid, fid as f64 * 5.0, &blob],
                        )
                        .unwrap();
                }
                connection.execute("INSERT INTO contour_tile_manifest(zoom_bucket,lat_bucket,lon_bucket,contour_count) VALUES(?1,?2,?3,150)",
                    params![spec.zoom_bucket, lat, lon]).unwrap();
            }
        }
    };
    insert(BASE_ZOOM, 1);
    let ctx = egui::Context::default();
    let wait_for = |source_zoom: f32| {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let result =
                load_srtm_region_for_view(Some(&root), center, center, 60.0, 3, 3, ctx.clone());
            if result.source_zoom == source_zoom
                && has_geometry(&result)
                && result.status.ready_assets == result.status.total_assets
            {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "source {source_zoom} did not load"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let base = wait_for(BASE_ZOOM);
    assert_eq!(base.tiles.len(), 9);
    assert!(base.tiles.iter().all(|t| t.contours.len() == 150));
    insert(60.0, 3);
    let fine = wait_for(60.0);
    assert!(covers_view(&fine, center, 60.0));
    // All decodes/merges completed; stop outstanding manifest refreshes before
    // fixture removal. Reset epochs also reject any late worker publications.
    blast_tile_caches();
    drop(connection);
    std::thread::sleep(Duration::from_millis(100));
    std::fs::remove_dir_all(root).unwrap();
}
