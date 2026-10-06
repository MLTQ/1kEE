use super::*;

fn lines(elevation: f32, west: f32, east: f32, lat: f32) -> Arc<Vec<ContourPath>> {
    Arc::new(vec![ContourPath {
        elevation_m: elevation,
        points: vec![GeoPoint { lon: west, lat }, GeoPoint { lon: east, lat }],
    }])
}
fn flattened(f: &Frame) -> Vec<ContourPath> {
    f.tiles
        .iter()
        .flat_map(|t| t.contours.iter().cloned())
        .collect()
}
fn fallback(contours: Arc<Vec<ContourPath>>) -> Arc<Frame> {
    frame(vec![LocalTileGeometry {
        id: tile_id(0, (0, 0)),
        bounds: residency::Bounds::from_contours(&contours),
        contours,
    }])
}
fn compose(bucket: i32, tiles: &Tiles, old: Option<Arc<Frame>>) -> Output {
    compose_resident(
        bucket,
        tiles,
        old,
        HashMap::new(),
        None,
        globe_residency::GPU_BUDGET,
    )
}
fn crossings(f: &Frame, lon: f32) -> Vec<f32> {
    flattened(f)
        .iter()
        .filter(|c| {
            c.points
                .windows(2)
                .any(|p| p[0].lon.min(p[1].lon) < lon && p[0].lon.max(p[1].lon) > lon)
        })
        .map(|c| c.elevation_m)
        .collect()
}
#[test]
fn overlapping_legacy_tiles_draw_once_and_keep_sparse_outer_coverage() {
    let tiles = HashMap::from([
        ((0, 0), lines(50., -2.2, 2.2, 0.)),
        ((0, 1), lines(100., -1.21, 3.19, 0.)),
    ]);
    let result = compose(1, &tiles, None);
    for x in [-2.1, -1.2, -0.7, 0.2, 0.7, 1.7, 2.7, 3.1] {
        assert_eq!(crossings(&result.frame, x).len(), 1, "coverage at {x}");
    }
    assert_eq!(crossings(&result.frame, 0.2), vec![50.]);
    assert_eq!(crossings(&result.frame, 0.7), vec![100.]);
}
#[test]
fn modern_and_empty_cores_replace_legacy_halos_without_bridging_gaps() {
    let tiles = HashMap::from([
        ((0, 0), lines(50., -2.2, 2.2, 0.)),
        ((0, 1), lines(100., 0.495, 1.485, 0.)),
        ((0, -1), Arc::new(vec![])),
    ]);
    let result = compose(1, &tiles, None);
    assert!(crossings(&result.frame, -0.8).is_empty());
    assert_eq!(crossings(&result.frame, 0.8), vec![100.]);
    assert_eq!(crossings(&result.frame, -1.8), vec![50.]);
    assert_eq!(crossings(&result.frame, 1.8), vec![50.]);
}
#[test]
fn zoom_fallback_survives_only_where_replacement_is_missing() {
    let tiles = HashMap::from([((0, 0), lines(50., -0.49, 0.49, 0.))]);
    let result = compose(1, &tiles, Some(fallback(lines(200., -3., 3., 0.))));
    assert_eq!(crossings(&result.frame, 0.2), vec![50.]);
    assert_eq!(crossings(&result.frame, 1.7), vec![200.]);
    assert_eq!(crossings(&result.frame, -1.7), vec![200.]);
    // Empty incoming cores also retire outgoing geometry.
    let empty = compose(
        1,
        &HashMap::from([((0, 0), Arc::new(vec![]))]),
        Some(result.frame),
    );
    assert!(crossings(&empty.frame, 0.2).is_empty());
}
#[test]
fn arriving_tiles_reuse_existing_geometry_and_reset_rejects_stale_merge() {
    let first = lines(50., -0.4, 0.4, 0.);
    let mut tiles = HashMap::from([((0, 0), first.clone())]);
    let old = compose(1, &tiles, None);
    assert!(Arc::ptr_eq(&old.frame.tiles[0].contours, &first));
    tiles.insert((0, 1), lines(100., 0.6, 1.4, 0.));
    let next = compose_resident(1, &tiles, None, old.pieces, None, usize::MAX);
    assert!(Arc::ptr_eq(&next.frame.tiles[0].contours, &first));
    let mut cache = GlobeRegionCache::default();
    cache.earth.frame = Some(old.frame.clone());
    cache.mark_tiles_changed();
    assert!(Arc::ptr_eq(cache.earth.frame.as_ref().unwrap(), &old.frame));
    cache.merge_in_flight = Some((0, 0));
    finish(&mut cache, 0, 0, 0, Some(next));
    assert!(cache.earth.frame.is_some());
    assert_ne!(cache.merged_revision, Some(cache.tiles_revision));
    cache.merge_in_flight = Some((0, 1));
    cache.reset_all();
    assert_eq!(cache.merge_in_flight, Some((0, 1)));
    finish(&mut cache, 0, 1, 0, Some(compose(1, &tiles, None)));
    assert!(cache.earth.frame.is_none());
    assert!(cache.merge_in_flight.is_none());
}
#[test]
fn repeated_zoom_handoffs_retire_replaced_fallback_arcs() {
    let mut picture = None;
    let mut retired = Vec::new();
    for i in 0..200 {
        if let Some(p) = &picture {
            retired.push(Arc::downgrade(p));
        }
        let output = compose(
            1,
            &HashMap::from([((0, 0), lines(i as f32, -0.4, 0.4, 0.))]),
            picture.take(),
        );
        assert!(output.fallback.is_none());
        assert_eq!(output.frame.tiles.len(), 1);
        picture = Some(output.frame);
        assert!(retired.iter().all(|w| w.upgrade().is_none()));
    }
    let mut state = State {
        frame: picture,
        ..Default::default()
    };
    state.transition(true);
    assert!(state.fallback.is_some());
    state.transition(false);
    assert!(state.fallback.is_none());
}
#[test]
fn stale_view_merge_cannot_reintroduce_evicted_geometry() {
    let mut cache = GlobeRegionCache {
        merge_in_flight: Some((0, 0)),
        ..Default::default()
    };
    cache.residency.revision = 1;
    finish(&mut cache, 0, 0, 0, Some(compose(1, &HashMap::new(), None)));
    assert!(cache.earth.frame.is_none());
    assert!(cache.merge_in_flight.is_none());
}
#[test]
fn retained_offscreen_sources_survive_but_old_fallback_retires_under_pressure() {
    let tiles = HashMap::from([
        ((0, 0), lines(50., -0.01, 0.01, 0.)),
        ((0, 50), lines(50., 49.3, 49.8, 0.)),
    ]);
    let old = compose(1, &tiles, None);
    assert_eq!(old.frame.tiles.len(), 2);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 400.));
    let view = crate::model::GlobeViewState::from_focus(GeoPoint { lat: 0., lon: 0. });
    let layout = crate::panels::world_map::globe_scene::GlobeLayout {
        center: rect.center(),
        radius: 60000.,
        focal_length: 2.,
        camera_distance: 3.,
    };
    let viewport = globe_residency::Viewport::new(&layout, &view, rect);
    let retained = compose_resident(
        1,
        &HashMap::new(),
        Some(old.frame.clone()),
        HashMap::new(),
        Some(viewport),
        56,
    );
    assert_eq!(retained.frame.tiles.len(), 2);
    let trimmed = compose_resident(
        1,
        &HashMap::new(),
        Some(old.frame),
        HashMap::new(),
        Some(viewport),
        28,
    );
    assert_eq!(trimmed.frame.tiles.len(), 1);
    assert!(trimmed.frame.tiles[0].contours[0].points[0].lon.abs() < 0.1);
}

#[test]
#[ignore = "read-only validation against installed Hilbert cache"]
fn real_moscow_mixed_cache_coverage_is_disjoint() {
    let path = PathBuf::from("/Volumes/Hilbert/Derived/terrain/srtm_focus_cache.sqlite");
    let request = |lat, lon| {
        let asset = srtm_focus_cache::FocusContourAsset {
            path: path.clone(),
            simplify_step: 1,
            zoom_bucket: 1,
            lat_bucket: lat,
            lon_bucket: lon,
        };
        (
            CacheKey {
                path: path.clone(),
                zoom_bucket: 1,
                lat_bucket: lat,
                lon_bucket: lon,
            },
            asset,
        )
    };
    let loaded = query_local_contours_batch(
        &path,
        &[request(53, 38), request(54, 38), request(54, 41)],
        usize::MAX,
    )
    .unwrap();
    let tiles: Tiles = loaded
        .into_iter()
        .map(|(k, c)| ((k.lat_bucket, k.lon_bucket), Arc::new(c)))
        .collect();
    assert_eq!(tiles.len(), 3);
    let bounds = tiles
        .iter()
        .map(|(&cell, c)| (cell, residency::Bounds::from_contours(c)))
        .collect();
    let regions = ownership(1, &tiles, &bounds);
    let boxes: Vec<_> = regions.values().flatten().collect();
    for (i, a) in boxes.iter().enumerate() {
        for b in &boxes[i + 1..] {
            assert!(intersection(**a, **b).is_none(), "double-owned region");
        }
    }
    let output = flattened(&compose(1, &tiles, None).frame);
    assert!(!output.is_empty());
    // The older tile's southern outer coverage must remain available even
    // though there is no decoded native tile for that area in this fixture.
    assert!(output.iter().flat_map(|c| &c.points).any(|p| p.lat < 51.0));
    eprintln!(
        "Moscow fixture: {} input paths -> {} disjoint clipped paths",
        tiles.values().map(|t| t.len()).sum::<usize>(),
        output.len()
    );
}
