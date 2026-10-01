use super::super::projection::LocalProjectionParams;
use super::*;
use crate::panels::world_map::{local_contour_pass::LocalTileId, srtm_focus_cache};

fn view(half: f32) -> Viewport {
    Viewport {
        body: ActiveBody::Earth,
        rect: egui::Rect::from_min_max(egui::pos2(-half, -half), egui::pos2(half, half)),
        projection: LocalProjectionParams {
            focus_lat: 0.0,
            focus_lon: 0.0,
            x_factor: 1.0,
            y_factor: 1.0,
            z_factor: 0.0,
            yaw_cos: 1.0,
            yaw_sin: 0.0,
            pitch_cos: 1.0,
            pitch_sin: 0.0,
            focus_center_x: 0.0,
            focus_center_y: 0.0,
            horizontal_scale: 1.0,
            ground_pitch_scale: 1.0,
            ground_depth_scale: 0.0,
            elevation_pitch_scale: 0.0,
            elevation_depth_scale: 0.0,
        },
    }
}

fn frame(zoom: f32, radius: i32) -> LocalContourLoad {
    let bucket = srtm_focus_cache::zoom_bucket_for_zoom(zoom);
    let mut tiles = Vec::new();
    for lat in -radius..=radius {
        for lon in -radius..=radius {
            tiles.push(LocalTileGeometry {
                id: LocalTileId {
                    zoom_bucket: bucket,
                    lat_bucket: lat,
                    lon_bucket: lon,
                },
                contours: Arc::new(Vec::new()),
                bounds: None,
            });
        }
    }
    LocalContourLoad {
        source_zoom: zoom,
        build_radius: radius,
        // Distinct pointers track which generation was displayed, even empty.
        contours: Some(Arc::new(Vec::new())),
        ready_buckets: tiles
            .iter()
            .map(|t| (t.id.lat_bucket, t.id.lon_bucket))
            .collect(),
        loading_progress: Default::default(),
        culled_buckets: Default::default(),
        status: srtm_focus_cache::FocusContourRegionStatus {
            ready_assets: tiles.len(),
            pending_assets: 0,
            total_assets: tiles.len(),
        },
        tiles,
    }
}

fn is_displaying(state: &Handoff, load: &LocalContourLoad) -> bool {
    Arc::ptr_eq(
        state.display.contours.as_ref().unwrap(),
        load.contours.as_ref().unwrap(),
    )
}

#[test]
fn zoom_in_waits_for_last_covering_child_and_last_gpu_upload() {
    let mut state = Handoff::default();
    let old = frame(8.0, 0); // 0.135-degree core
    let full = frame(12.0, 1); // nine 0.072-degree cores
    let mut partial = full.clone();
    partial
        .tiles
        .retain(|t| (t.id.lat_bucket, t.id.lon_bucket) != (1, 1));
    state.update(None, view(0.1), 12.0, &old, vec![], true);
    state.update(None, view(0.1), 12.0, &partial, vec![], true);
    assert!(is_displaying(&state, &old));
    state.update(None, view(0.1), 12.0, &full, vec![], false);
    assert!(is_displaying(&state, &old));
    state.update(None, view(0.1), 12.0, &full, vec![], true);
    assert!(is_displaying(&state, &full));
}

#[test]
fn zoom_out_and_rapid_reversal_never_accept_a_partial_replacement() {
    let mut state = Handoff::default();
    let fine = frame(12.0, 1);
    let wide = frame(8.0, 1);
    let mut incomplete = wide.clone();
    incomplete.tiles.retain(|t| t.id.lon_bucket != 1);
    state.update(None, view(0.2), 8.0, &fine, vec![], true);
    state.update(None, view(0.2), 8.0, &incomplete, vec![], true);
    assert!(is_displaying(&state, &fine));
    // The next target can change again before either replacement completes.
    let mut deepest = frame(35.0, 2);
    deepest.tiles.clear();
    state.update(None, view(0.02), 35.0, &deepest, vec![], true);
    assert!(is_displaying(&state, &fine));
    state.update(None, view(0.2), 8.0, &wide, vec![], true);
    assert!(is_displaying(&state, &wide));
}

#[test]
fn unknown_ready_counts_do_not_replace_terrain_but_decoded_empty_cells_do() {
    let old = frame(8.0, 0);
    let mut next = frame(12.0, 1);
    let saved = next.tiles.clone();
    next.tiles.clear(); // UI can report unavailable source cells as ready.
    assert!(!preserves_coverage(&old.tiles, &next, view(0.1), 12.0));
    next.tiles = saved;
    assert!(preserves_coverage(&old.tiles, &next, view(0.1), 12.0));
    next.ready_buckets.remove(&(0, 0)); // decoded, not published yet
    assert!(!preserves_coverage(&old.tiles, &next, view(0.1), 12.0));
}

#[test]
fn zoom_into_part_of_an_old_tile_does_not_wait_for_offscreen_children() {
    let old = frame(0.5, 0);
    let next = frame(35.0, 2);
    assert!(preserves_coverage(&old.tiles, &next, view(0.025), 35.0));
}

#[test]
fn offscreen_tiles_and_source_changes_release_retained_geometry() {
    let mut state = Handoff::default();
    let old = frame(8.0, 0);
    state.update(Some(Path::new("a")), view(0.1), 8.0, &old, vec![], true);
    let mut empty = frame(12.0, 0);
    empty.tiles.clear();
    empty.contours = None;
    let mut moved = view(0.1);
    moved.projection.focus_lon = 20.0;
    state.update(Some(Path::new("a")), moved, 12.0, &empty, vec![], true);
    assert!(state.display.contours.is_none());
    state.update(Some(Path::new("a")), view(0.1), 8.0, &old, vec![], true);
    state.update(Some(Path::new("b")), view(0.1), 12.0, &empty, vec![], true);
    assert!(state.display.contours.is_none());
    state.update(None, view(0.1), 8.0, &old, vec![], true);
    let mut moon = view(0.1);
    moon.body = ActiveBody::Moon;
    state.update(None, moon, 12.0, &empty, vec![], true);
    assert!(state.display.contours.is_none());
}

#[test]
fn rotated_view_accepts_deliberately_culled_replacement_cells() {
    let old = frame(8.0, 0);
    let mut next = frame(12.0, 1);
    next.tiles
        .retain(|t| (t.id.lat_bucket, t.id.lon_bucket) != (1, 1));
    next.culled_buckets.insert((1, 1));
    let mut rotated = view(0.1);
    rotated.projection.yaw_cos = std::f32::consts::FRAC_1_SQRT_2;
    rotated.projection.yaw_sin = std::f32::consts::FRAC_1_SQRT_2;
    assert!(preserves_coverage(&old.tiles, &next, rotated, 12.0));
}

#[test]
fn visible_terrain_outside_nominal_ground_extent_is_not_discarded() {
    let old = frame(8.0, 0);
    let fine = frame(50.0, 2);
    // A shallow-pitch view can still see old terrain beyond zoom 60's usual
    // 0.0125-degree envelope. Merely clipping to that envelope loses it.
    assert!(!preserves_coverage(&old.tiles, &fine, view(0.1), 60.0));
    assert!(preserves_coverage(&old.tiles, &fine, view(0.01), 60.0));
}

#[test]
fn inverse_coverage_bounds_include_projected_elevated_corners() {
    let mut camera = view(0.1);
    camera.projection.z_factor = 0.01;
    camera.projection.pitch_cos = 0.7;
    camera.projection.pitch_sin = (1.0_f32 - 0.49).sqrt();
    camera.projection.elevation_pitch_scale = 0.3;
    camera.projection.elevation_depth_scale = 0.2;
    camera.projection.yaw_cos = 0.6;
    camera.projection.yaw_sin = 0.8;
    let bounds = visible_ground_bounds(camera, -10.0, 100.0).unwrap();
    for lon in -100..100 {
        for lat in -100..100 {
            let point = crate::model::GeoPoint {
                lon: lon as f32 / 100.0,
                lat: lat as f32 / 100.0,
            };
            for height in [-10.0, 0.0, 50.0, 100.0] {
                let (x, y, _) = camera.projection.project_logical(point, height);
                if camera.rect.contains(egui::pos2(x, y)) {
                    assert!((bounds.min[0]..=bounds.max[0]).contains(&point.lon));
                    assert!((bounds.min[1]..=bounds.max[1]).contains(&point.lat));
                }
            }
        }
    }
}
