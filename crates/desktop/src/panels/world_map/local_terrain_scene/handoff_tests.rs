use super::super::projection::LocalProjectionParams;
use super::*;
use crate::panels::world_map::{
    contour_asset::residency::Bounds, local_contour_pass::LocalTileId, srtm_focus_cache,
};

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
        detail: None,
    }
}

fn complete(state: &mut Handoff) {
    let ticket = state.revision + 100;
    state.working = Some(ticket);
    let result = composition::compose(
        state.identity.as_ref().unwrap().1,
        state.sources.values().cloned().collect(),
        state.pieces.clone(),
    );
    state.finish(
        ticket,
        state.epoch,
        state.generation,
        state.revision,
        Some(result),
    );
}

fn fallback(detail: LocalContourLoad) -> LocalContourLoad {
    let mut base = frame(12.0, 0);
    base.detail = Some(crate::panels::world_map::contour_asset::LocalContourDetail {
        source_zoom: detail.source_zoom,
        tiles: detail.tiles,
    });
    base
}

#[test]
fn small_pan_with_a_missing_fine_edge_keeps_visible_detail_and_its_priority() {
    let mut state = Handoff::default();
    let fine = frame(60.0, 0);
    let fine_id = fine.tiles[0].id;
    let fine_lines = fine.tiles[0].contours.clone();
    state.observe(None, view(0.02), &fine);
    complete(&mut state);
    state.publish(state.candidate(), vec![], false, true);
    let generation = state.generation;

    let mut panned = view(0.02);
    panned.projection.focus_lon = 0.001;
    // A refreshed manifest can briefly omit even the already displayed tile.
    let mut missing = frame(60.0, 0);
    missing.tiles.clear();
    state.observe(None, panned, &fallback(missing));
    complete(&mut state);
    state.publish(state.candidate(), vec![], false, true);
    assert_eq!(state.generation, generation);
    let retained = state.display.tiles.iter().find(|t| t.id == fine_id).unwrap();
    assert!(Arc::ptr_eq(&retained.contours, &fine_lines));
    assert_eq!(state.display.tiles.len(), 2); // base only outside the fine core

    // An intentional zoom-out must still replace fine terrain when ready.
    state.observe(None, view(0.1), &frame(12.0, 0));
    complete(&mut state);
    state.publish(state.candidate(), vec![], false, true);
    assert_eq!(state.display.tiles.len(), 1);
    assert_eq!(state.display.tiles[0].id.zoom_bucket, 6);
}

#[test]
fn partial_detail_is_admitted_over_base_without_waiting_for_whole_view() {
    let mut state = Handoff::default();
    let mut missing = frame(60.0, 0);
    missing.tiles.clear();
    state.observe(None, view(0.02), &fallback(missing));
    complete(&mut state);
    state.publish(state.candidate(), vec![], false, true);
    assert_eq!(state.display.tiles[0].id.zoom_bucket, 6);
    let generation = state.generation;

    let load = fallback(frame(60.0, 0));
    state.observe(None, view(0.02), &load);
    complete(&mut state);
    assert_eq!(state.candidate().tiles.len(), 2);
    assert_eq!(state.generation, generation);
    let fine_id = load.detail.as_ref().unwrap().tiles[0].id;
    let fine_core = composition::core(ActiveBody::Earth, fine_id);
    assert_eq!(state.pieces[&fine_id].regions, vec![fine_core]);
    let base_id = load.tiles[0].id;
    assert_eq!(state.pieces[&base_id].regions.len(), 4);
}

#[test]
fn fully_covered_base_arrivals_do_not_schedule_composition_every_frame() {
    let mut state = Handoff::default();
    // Hosted bucket seven has a larger core than the SRTM base; one ready
    // hosted tile can completely replace this base tile while edges load.
    let load = fallback(frame(16.0, 0));
    state.observe(None, view(0.1), &load);
    complete(&mut state);
    state.publish(state.candidate(), vec![], false, true);
    assert_eq!(state.display.tiles.len(), 1);
    assert_eq!(state.display.tiles[0].id.zoom_bucket, 7);
    let revision = state.revision;
    for _ in 0..3 {
        state.observe(None, view(0.1), &load);
        assert_eq!(state.revision, revision);
        assert_eq!(state.revision, state.built_revision);
    }
    let mut away = view(0.1);
    away.projection.focus_lon = 1.0;
    state.observe(None, away, &load);
    assert!(state.sources.is_empty());
    assert!(state.display.tiles.is_empty());
}

#[test]
fn partial_ready_detail_is_composed_without_waiting_for_missing_edge_cells() {
    let mut state = Handoff::default();
    let old = frame(8.0, 0);
    state.observe(None, view(0.1), &old);
    complete(&mut state);
    state.publish(state.candidate(), vec![], false, true);
    let old_picture = state.display.contours.clone().unwrap();
    let fine = frame(12.0, 0); // surrounding finer cells are missing
    state.observe(None, view(0.1), &fine);
    complete(&mut state);
    assert_eq!(state.candidate().tiles.len(), 2); // finer core plus coarse remainder
    state.publish(state.candidate(), vec![], true, false);
    assert!(Arc::ptr_eq(
        &old_picture,
        state.display.contours.as_ref().unwrap()
    ));
    state.publish(state.candidate(), vec![], true, true);
    assert_eq!(state.display.tiles.len(), 2);
    assert!(!Arc::ptr_eq(
        &old_picture,
        state.display.contours.as_ref().unwrap()
    ));
}

#[test]
fn read_arrivals_do_not_replace_a_frame_while_it_is_uploading() {
    let mut state = Handoff::default();
    state.observe(None, view(0.2), &frame(12.0, 0));
    complete(&mut state);
    let uploading = state.candidate();
    state.observe(None, view(0.2), &frame(12.0, 1));
    assert_eq!(state.candidate().id, uploading.id);
    assert_eq!(state.candidate().tiles.len(), 1);
    state.publish(uploading, vec![], true, true);
    assert_ne!(state.revision, state.built_revision);
    complete(&mut state);
    assert_eq!(state.candidate().tiles.len(), 9);
}

#[test]
fn worker_finishing_between_prepare_and_select_cannot_lose_its_result() {
    let mut state = Handoff::default();
    state.observe(None, view(0.2), &frame(12.0, 0));
    let before_worker_finished = state.candidate();
    complete(&mut state);
    let new_frame = state.candidate().id;
    state.publish(before_worker_finished, vec![], false, true);
    assert_eq!(state.prepared.as_ref().unwrap().id, new_frame);
    state.publish(state.candidate(), vec![], false, true);
    assert_eq!(state.display.frame_id, new_frame);
}

#[test]
fn zoom_reversal_rejects_old_worker_without_opening_a_second_worker_slot() {
    let mut state = Handoff::default();
    state.observe(None, view(0.2), &frame(12.0, 0));
    state.working = Some(7);
    let (epoch, generation, revision) = (state.epoch, state.generation, state.revision);
    let stale = composition::compose(
        ActiveBody::Earth,
        state.sources.values().cloned().collect(),
        HashMap::new(),
    );
    state.observe(None, view(0.2), &frame(8.0, 0));
    assert_eq!(state.working, Some(7));
    state.finish(7, epoch, generation, revision, Some(stale));
    assert!(state.prepared.is_none());
    assert!(state.working.is_none());
    complete(&mut state);
    state.publish(state.candidate(), vec![], false, true);
    state.observe(None, view(0.2), &frame(12.0, 0));
    complete(&mut state);
    assert_eq!(state.candidate().tiles.len(), 2);
}

#[test]
fn no_source_progress_is_not_replacement_coverage() {
    let mut state = Handoff::default();
    state.observe(None, view(0.2), &frame(8.0, 0));
    complete(&mut state);
    state.publish(state.candidate(), vec![], false, true);
    let mut unavailable = frame(12.0, 1);
    unavailable.tiles.clear(); // ready counts can still report no-source cells
    state.observe(None, view(0.2), &unavailable);
    complete(&mut state);
    assert_eq!(state.candidate().tiles.len(), 1);
    assert_eq!(state.candidate().tiles[0].id.zoom_bucket, 5);
}

#[test]
fn root_body_reset_and_offscreen_pan_release_display_and_reject_late_workers() {
    for change in 0..4 {
        let mut state = Handoff::default();
        state.observe(Some(Path::new("a")), view(0.2), &frame(8.0, 0));
        complete(&mut state);
        state.publish(state.candidate(), vec![], false, true);
        let epoch = state.epoch;
        state.working = Some(42);
        let mut camera = view(0.2);
        let mut absent = frame(12.0, 0);
        absent.tiles.clear();
        absent.contours = None;
        match change {
            0 => state.observe(Some(Path::new("b")), camera, &absent),
            1 => {
                camera.body = ActiveBody::Moon;
                state.observe(Some(Path::new("a")), camera, &absent);
            }
            2 => state.clear(),
            _ => {
                camera.projection.focus_lon = 30.0;
                state.observe(Some(Path::new("a")), camera, &absent);
            }
        }
        assert!(state.display.tiles.is_empty());
        assert!(state.display.contours.is_none());
        state.finish(
            42,
            epoch,
            0,
            0,
            Some(composition::compose(
                ActiveBody::Earth,
                vec![],
                HashMap::new(),
            )),
        );
        assert!(state.prepared.is_none());
        assert!(state.working.is_none());
    }
}

#[test]
fn elevated_core_is_retained_when_it_projects_into_the_view() {
    let mut load = frame(12.0, 0);
    load.tiles[0].id.lat_bucket = -30;
    load.tiles[0].bounds = Some(Bounds {
        min: [-0.01, -2.2, 2.16],
        max: [0.01, -2.1, 2.16],
    });
    load.ready_buckets = HashSet::from([(-30, 0)]);
    let mut camera = view(0.1);
    camera.projection.z_factor = 1.0;
    camera.projection.elevation_depth_scale = 1.0;
    let mut state = Handoff::default();
    state.observe(None, camera, &load);
    assert_eq!(state.sources.len(), 1);
}
