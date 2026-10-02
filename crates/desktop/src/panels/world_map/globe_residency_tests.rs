use super::*;

fn viewport(lon: f32, radius: f32) -> Viewport {
    let layout = GlobeLayout {
        center: egui::pos2(200.0, 200.0),
        radius,
        focal_length: 2.0,
        camera_distance: 3.0,
    };
    Viewport::new(
        &layout,
        &GlobeViewState::from_focus(GeoPoint { lat: 0.0, lon }),
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(400.0, 400.0)),
    )
}

#[test]
fn culling_rejects_offscreen_and_backside_but_keeps_visible_crossings() {
    let v = viewport(0.0, 6000.0);
    assert!(v.intersects(Rect {
        min_lat: -0.5,
        max_lat: 0.5,
        min_lon: -0.5,
        max_lon: 0.5
    }));
    assert!(!v.intersects(Rect {
        min_lat: 30.0,
        max_lat: 31.0,
        min_lon: 0.0,
        max_lon: 1.0
    }));
    assert!(!v.intersects(Rect {
        min_lat: -0.5,
        max_lat: 0.5,
        min_lon: 179.0,
        max_lon: 181.0
    }));
    assert!(v.intersects(Rect {
        min_lat: -0.5,
        max_lat: 0.5,
        min_lon: -20.0,
        max_lon: 20.0
    }));
    assert!(viewport(179.9, 6000.0).intersects(Rect {
        min_lat: -0.5,
        max_lat: 0.5,
        min_lon: 179.5,
        max_lon: 180.5
    }));
}

#[test]
fn visible_point_sampling_never_falsely_culls_a_patch() {
    for lat in [-80.0_f32, -30.0, 0.0, 55.0, 85.0] {
        for lon in [-179.9, -90.0, 0.0, 37.6, 179.9] {
            let layout = GlobeLayout {
                center: egui::pos2(280.0, 230.0),
                radius: 1800.0,
                focal_length: 2.0,
                camera_distance: 3.0,
            };
            let state = GlobeViewState::from_focus(GeoPoint { lat, lon });
            let rect = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(640.0, 400.0));
            let v = Viewport::new(&layout, &state, rect);
            // All patches surrounding the camera focus must remain possible even
            // across +/-180 or at high latitude; plane culling is conservative.
            assert!(v.intersects(Rect {
                min_lat: f64::from(lat) - 0.5,
                max_lat: f64::from(lat) + 0.5,
                min_lon: f64::from(lon) - 0.5,
                max_lon: f64::from(lon) + 0.5
            }));
            // Independently project points like the shader, including a
            // deliberately asymmetric viewport and both terrain radii.
            for point_lat in (-90..=90).step_by(3) {
                for point_lon in (-180..=180).step_by(3) {
                    for radius in [1.015, 1.02] {
                        let la = (point_lat as f32).to_radians();
                        let lo = (point_lon as f32).to_radians();
                        let p = [la.cos() * lo.cos(), la.sin(), la.cos() * lo.sin()];
                        let x = (p[0] * state.yaw.cos() + p[2] * state.yaw.sin()) * radius;
                        let z = -p[0] * state.yaw.sin() + p[2] * state.yaw.cos();
                        let y = (p[1] * state.pitch.cos() - z * state.pitch.sin()) * radius;
                        let z = (p[1] * state.pitch.sin() + z * state.pitch.cos()) * radius;
                        let depth = layout.camera_distance - z;
                        let scale = layout.radius * layout.focal_length / depth;
                        let screen = layout.center - egui::vec2(x, y) * scale;
                        if z >= 0.0 && depth > 0.05 && rect.contains(screen) {
                            assert!(
                                v.intersects(Rect {
                                    min_lat: f64::from(point_lat) - 0.5,
                                    max_lat: f64::from(point_lat) + 0.5,
                                    min_lon: f64::from(point_lon) - 0.5,
                                    max_lon: f64::from(point_lon) + 0.5,
                                }),
                                "visible point {point_lat}/{point_lon} at focus {lat}/{lon}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn long_tour_releases_old_source_arcs_and_keeps_metadata_bounded() {
    let mut cache = GlobeRegionCache::default();
    let mut old = Vec::new();
    for stop in 0..120 {
        let lon = -150.0 + stop as f32 * 2.5;
        let cell = (0, (lon / 0.99).round() as i32);
        cache.residency.view = Some(viewport(lon, 6000.0));
        cache.residency.configure(1, cell);
        prune(&mut cache);
        let contours = Arc::new(vec![ContourPath {
            elevation_m: 50.0,
            points: vec![
                GeoPoint { lat: 0.0, lon },
                GeoPoint {
                    lat: 0.1,
                    lon: lon + 0.1,
                },
            ],
        }]);
        old.push(Arc::downgrade(&contours));
        cache
            .residency
            .record(cell, residency::Bounds::from_contours(&contours));
        cache.tiles.insert(cell, contours);
        cache.order.push(cell);
        assert!(cache.tiles.len() < 8);
        assert!(cache.residency.bounds.len() <= 23 * 23);
        assert!(old.iter().filter(|w| w.upgrade().is_some()).count() < 8);
    }
    assert!(old[0].upgrade().is_none());
}

#[test]
fn reader_completing_after_a_long_jump_cannot_restore_old_tiles() {
    let mut cache = GlobeRegionCache::default();
    cache.load_in_flight = Some(cache.load_epoch);
    cache.residency.view = Some(viewport(90.0, 6000.0));
    cache.residency.configure(1, (0, 91));
    let epoch = cache.load_epoch;
    let contours = vec![ContourPath {
        elevation_m: 50.0,
        points: vec![
            GeoPoint { lat: 0.0, lon: 0.0 },
            GeoPoint { lat: 0.1, lon: 0.1 },
        ],
    }];
    let cache = Mutex::new(cache);
    finish_globe_read(
        &cache,
        epoch,
        &[],
        Some(vec![(
            CacheKey {
                path: PathBuf::new(),
                zoom_bucket: 1,
                lat_bucket: 0,
                lon_bucket: 0,
            },
            contours,
        )]),
    );
    assert!(cache.lock().unwrap().tiles.is_empty());
}
