use super::*;

fn lines(elevation: f32, west: f32, east: f32, lat: f32) -> Arc<Vec<ContourPath>> {
    Arc::new(vec![ContourPath {
        elevation_m: elevation,
        points: vec![GeoPoint { lon: west, lat }, GeoPoint { lon: east, lat }],
    }])
}

fn crossings(contours: &[ContourPath], lon: f32) -> Vec<f32> {
    contours
        .iter()
        .filter(|line| {
            line.points
                .windows(2)
                .any(|p| p[0].lon.min(p[1].lon) < lon && p[0].lon.max(p[1].lon) > lon)
        })
        .map(|c| c.elevation_m)
        .collect()
}

#[test]
fn overlapping_legacy_tiles_draw_once_and_keep_sparse_outer_coverage() {
    let tiles = HashMap::from([
        ((0, 0), lines(50.0, -2.2, 2.2, 0.0)),
        ((0, 1), lines(100.0, -1.21, 3.19, 0.0)),
    ]);
    let result = compose(1, &tiles, None);
    for x in [-2.1, -1.2, -0.7, 0.2, 0.7, 1.7, 2.7, 3.1] {
        assert_eq!(crossings(&result, x).len(), 1, "coverage/overlap at {x}");
    }
    assert_eq!(crossings(&result, 0.2), vec![50.0]);
    assert_eq!(crossings(&result, 0.7), vec![100.0]);
}

#[test]
fn modern_and_empty_cores_replace_legacy_halos_without_bridging_gaps() {
    let tiles = HashMap::from([
        ((0, 0), lines(50.0, -2.2, 2.2, 0.0)),
        ((0, 1), lines(100.0, 0.495, 1.485, 0.0)),
        ((0, -1), Arc::new(vec![])),
    ]);
    let result = compose(1, &tiles, None);
    assert!(crossings(&result, -0.8).is_empty());
    assert_eq!(crossings(&result, 0.8), vec![100.0]);
    assert_eq!(crossings(&result, -1.8), vec![50.0]);
    assert_eq!(crossings(&result, 1.8), vec![50.0]);
}

#[test]
fn zoom_fallback_survives_only_where_replacement_is_missing() {
    let fallback = lines(200.0, -3.0, 3.0, 0.0);
    let tiles = HashMap::from([((0, 0), lines(50.0, -0.49, 0.49, 0.0))]);
    let result = compose(1, &tiles, Some(fallback));
    assert_eq!(crossings(&result, 0.2), vec![50.0]);
    assert_eq!(crossings(&result, 1.7), vec![200.0]);
    assert_eq!(crossings(&result, -1.7), vec![200.0]);
}

#[test]
fn arriving_tiles_preserve_published_arc_and_reset_rejects_stale_merge() {
    let mut cache = GlobeRegionCache::default();
    let old = lines(50.0, -0.4, 0.4, 0.0);
    cache.merged = Some(old.clone());
    cache.mark_tiles_changed();
    assert!(Arc::ptr_eq(cache.merged.as_ref().unwrap(), &old));
    let epoch = cache.load_epoch;
    cache.merge_in_flight = Some((epoch, 0));
    finish(
        &mut cache,
        epoch,
        0,
        0,
        Some(Output {
            contours: old.clone(),
            fallback: None,
        }),
    );
    assert!(cache.merged.is_some()); // intermediate revision still publishes
    assert_ne!(cache.merged_revision, Some(cache.tiles_revision));
    cache.merge_in_flight = Some((epoch, 1));
    cache.reset_all();
    assert_eq!(cache.merge_in_flight, Some((epoch, 1))); // one occupied worker
    finish(
        &mut cache,
        epoch,
        1,
        0,
        Some(Output {
            contours: old,
            fallback: None,
        }),
    );
    assert!(cache.merged.is_none());
    assert!(cache.merge_in_flight.is_none());
}

#[test]
fn repeated_zoom_and_event_hops_do_not_accumulate_fallback_history() {
    let mut picture = None;
    let mut retired = Vec::new();
    for stop in 0..200 {
        let lon = -100.0 + stop as f32 * 0.9;
        let cell = (0, (lon / 0.99).round() as i32);
        let core = composition::core(ActiveBody::Earth, tile_id(1, cell));
        let source = lines(
            50.0,
            core.min_lon as f32 + 0.01,
            core.max_lon as f32 - 0.01,
            0.0,
        );
        if let Some(p) = &picture {
            retired.push(Arc::downgrade(p));
        }
        let output = compose_visible(
            1,
            &HashMap::from([(cell, source)]),
            picture.take(),
            Some(&[core]),
        );
        assert!(output.fallback.is_none()); // native tile covers the entire visible cell
        assert!(output.contours.len() <= 1);
        picture = Some(output.contours);
        assert!(retired.iter().all(|w| w.upgrade().is_none()));
    }
}

#[test]
fn stale_view_merge_cannot_reintroduce_evicted_geometry() {
    let mut cache = GlobeRegionCache {
        merge_in_flight: Some((0, 0)),
        ..Default::default()
    };
    cache.residency.revision = 1;
    finish(
        &mut cache,
        0,
        0,
        0,
        Some(Output {
            contours: lines(50.0, -3.0, 3.0, 0.0),
            fallback: None,
        }),
    );
    assert!(cache.merged.is_none());
    assert!(cache.merge_in_flight.is_none());
}

#[test]
fn retained_sources_outside_the_current_window_still_compose() {
    let tiles = HashMap::from([
        ((0, 0), lines(50.0, -0.4, 0.4, 0.0)),
        ((0, 40), lines(50.0, 39.3, 39.8, 0.0)),
    ]);
    let output = compose_resident(1, &tiles, None, None, globe_residency::GPU_BUDGET);
    assert!(
        output
            .contours
            .iter()
            .any(|p| p.points.iter().any(|p| p.lon > 39.0))
    );
    assert!(
        output
            .contours
            .iter()
            .any(|p| p.points.iter().any(|p| p.lon < 1.0))
    );
}

#[test]
fn fallback_only_retires_offscreen_paths_under_budget_pressure() {
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0));
    let view = crate::model::GlobeViewState::from_focus(GeoPoint { lat: 0.0, lon: 0.0 });
    let layout = crate::panels::world_map::globe_scene::GlobeLayout {
        center: rect.center(),
        radius: 60000.0,
        focal_length: 2.0,
        camera_distance: 3.0,
    };
    let viewport = globe_residency::Viewport::new(&layout, &view, rect);
    let mut fallback = (*lines(50.0, -0.01, 0.01, 0.0)).clone();
    fallback.extend(lines(50.0, 50.0, 50.1, 0.0).iter().cloned());
    let fallback = Arc::new(fallback);
    let retained = compose_resident(
        1,
        &HashMap::new(),
        Some(fallback.clone()),
        Some(viewport),
        56,
    );
    assert_eq!(retained.contours.len(), 2);
    let trimmed = compose_resident(1, &HashMap::new(), Some(fallback), Some(viewport), 28);
    assert_eq!(trimmed.contours.len(), 1);
    assert!(trimmed.contours[0].points[0].lon.abs() < 0.1);
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
    let regions = ownership(1, &tiles);
    let boxes: Vec<_> = regions.values().flatten().collect();
    for (i, a) in boxes.iter().enumerate() {
        for b in &boxes[i + 1..] {
            assert!(intersection(**a, **b).is_none(), "double-owned region");
        }
    }
    let output = compose(1, &tiles, None);
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
