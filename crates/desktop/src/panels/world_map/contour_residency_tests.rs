use super::*;

fn viewport() -> Viewport {
    Viewport {
        body: ActiveBody::Earth,
        rect: egui::Rect::from_min_max(egui::pos2(-1.0, -1.0), egui::pos2(1.0, 1.0)),
        projection: LocalProjectionParams {
            focus_lat: 0.0,
            focus_lon: 0.0,
            x_factor: 1.0,
            y_factor: 1.0,
            z_factor: 1.0,
            yaw_cos: 1.0,
            yaw_sin: 0.0,
            pitch_cos: 1.0,
            pitch_sin: 0.0,
            focus_center_x: 0.0,
            focus_center_y: 0.0,
            horizontal_scale: 1.0,
            ground_pitch_scale: 1.0,
            ground_depth_scale: 0.0,
            elevation_pitch_scale: 1.0,
            elevation_depth_scale: 1.0,
        },
    }
}

fn key(lon_bucket: i32) -> CacheKey {
    CacheKey {
        path: "residency.sqlite".into(),
        zoom_bucket: 6,
        lat_bucket: 0,
        lon_bucket,
    }
}

fn line(a: (f32, f32), b: (f32, f32), elevation_m: f32) -> Vec<ContourPath> {
    vec![ContourPath {
        elevation_m,
        points: vec![
            GeoPoint { lon: a.0, lat: a.1 },
            GeoPoint { lon: b.0, lat: b.1 },
        ],
    }]
}

#[test]
fn keeps_crossing_lines_and_rejects_fully_offscreen_geometry() {
    let view = viewport();
    assert!(view.intersects(Bounds::from_contours(&line((-2.0, 0.0), (2.0, 0.0), 0.0)).unwrap()));
    assert!(!view.intersects(Bounds::from_contours(&line((2.0, 0.0), (3.0, 0.0), 0.0)).unwrap()));
    // Ground is below the screen; elevated terrain projects back into view.
    assert!(view.intersects(Bounds::from_contours(&line((0.0, -3.0), (0.5, -3.0), 3.0)).unwrap()));
    assert!(!view.intersects(Bounds::from_contours(&line((0.0, -3.0), (0.5, -3.0), 0.0)).unwrap()));
}

#[test]
fn rotated_box_is_rejected_even_when_its_screen_aabb_overlaps() {
    let mut view = viewport();
    view.projection.yaw_cos = std::f32::consts::FRAC_1_SQRT_2;
    view.projection.yaw_sin = std::f32::consts::FRAC_1_SQRT_2;
    // x-y projection becomes a thin diagonal beside the upper-right corner.
    let bounds = Bounds {
        min: [-1.0, -1.9, 0.0],
        max: [1.0, -1.8, 0.0],
    };
    assert!(!view.intersects(bounds));
    view.rect = view.rect.expand(1.0);
    assert!(view.intersects(bounds));
}

#[test]
fn oblique_visibility_keeps_projected_corners_across_camera_angles() {
    let bounds = Bounds {
        min: [-0.1, -0.1, -0.4],
        max: [0.1, 0.1, 2.0],
    };
    for yaw in [0.0_f32, 0.7, 2.4, 4.7] {
        for pitch in [0.2_f32, 0.8, 1.55] {
            let mut view = viewport();
            view.projection.yaw_cos = yaw.cos();
            view.projection.yaw_sin = yaw.sin();
            view.projection.pitch_cos = pitch.cos();
            view.projection.pitch_sin = pitch.sin();
            assert!(view.intersects(bounds));
        }
    }
}

#[test]
fn pan_drops_geometry_late_results_stay_evicted_and_return_pan_reloads() {
    let k = key(0);
    let geometry = line((0.0, 0.0), (0.5, 0.0), 0.0);
    let mut state = LocalRegionCache::default();
    state.residency.viewport = Some(viewport());
    state.residency.set_window(0, 0, 6);
    state.load_in_flight = Some(state.load_epoch);
    let cache = Mutex::new(state);
    assert!(reader::publish_tile(&cache, 0, k.clone(), geometry.clone()));
    let weak = Arc::downgrade(cache.lock().unwrap().entries.get(&k).unwrap());
    {
        let mut state = cache.lock().unwrap();
        state
            .residency
            .viewport
            .as_mut()
            .unwrap()
            .projection
            .focus_lon = 10.0;
        prune(&mut state);
        assert!(state.entries.is_empty());
        assert!(!state.residency.wanted(&k));
    }
    assert!(weak.upgrade().is_none());
    assert!(reader::publish_tile(&cache, 0, k.clone(), geometry));
    let mut state = cache.lock().unwrap();
    assert!(state.entries.is_empty()); // late result didn't resurrect the tile
    state.load_in_flight = None;
    let asset = srtm_focus_cache::FocusContourAsset {
        path: k.path.clone(),
        zoom_bucket: 6,
        lat_bucket: 0,
        lon_bucket: 0,
        simplify_step: 1,
    };
    assert!(begin_local_read(&mut state, std::slice::from_ref(&asset), 0, 0).is_none());
    state.residency.viewport = Some(viewport());
    assert!(begin_local_read(&mut state, &[asset], 0, 0).is_some());
}

#[test]
fn metadata_and_empty_tile_residency_are_bounded() {
    let mut state = Residency {
        viewport: Some(viewport()),
        ..Default::default()
    };
    state.record(key(0), None);
    state.record(key(100), None);
    state.set_window(0, 0, 6);
    assert!(state.wanted(&key(0)));
    assert!(!state.wanted(&key(100)));
    assert_eq!(state.bounds.len(), 1);
    state.clear_geometry_metadata();
    assert!(state.bounds.is_empty());
}

#[test]
fn culled_disk_tiles_finish_progress_without_being_reloaded() {
    let k = key(0);
    let mut state = LocalRegionCache::default();
    let mut view = viewport();
    view.projection.focus_lon = 10.0;
    state.residency.viewport = Some(view);
    state.residency.record(
        k.clone(),
        Bounds::from_contours(&line((0.0, 0.0), (0.5, 0.0), 0.0)),
    );
    state.manifest_requested_key = Some(LocalManifestKey {
        root: None,
        center_lat_bucket: 0,
        center_lon_bucket: 0,
        zoom_bucket: 6,
        prefetch_radius: 0,
        build_radius: 0,
    });
    let assets = LocalManifestAssetViews {
        assets: vec![srtm_focus_cache::FocusContourAsset {
            path: k.path,
            zoom_bucket: 6,
            lat_bucket: 0,
            lon_bucket: 0,
            simplify_step: 1,
        }],
        ..Default::default()
    };
    let snapshot = loading::snapshot(
        &Mutex::new(state),
        &assets,
        srtm_focus_cache::LocalContourRegionState {
            ready_buckets: HashSet::from([(0, 0)]),
            status: srtm_focus_cache::FocusContourRegionStatus {
                ready_assets: 1,
                total_assets: 1,
                pending_assets: 0,
            },
        },
    );
    assert_eq!(snapshot.status.ready_assets, snapshot.status.total_assets);
    assert_eq!(snapshot.fractions[&(0, 0)], 1.0);
    assert!(snapshot.culled_buckets.contains(&(0, 0)));
}
