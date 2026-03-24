use super::local_terrain_scene;
use crate::model::AppModel;
use crate::osm_ingest;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub(super) fn ensure_visible_road_layers(model: &mut AppModel, local_terrain_mode: bool) {
    if !local_terrain_mode || (!model.show_major_roads && !model.show_minor_roads) {
        return;
    }

    // Rate-limit: only attempt queue checks twice per second.  The actual
    // queue check is now O(1) thanks to in-memory caches, but calling
    // ensure_runtime_store (which opens SQLite) on every frame is still
    // wasteful when nothing has changed.
    static LAST_CHECK: OnceLock<Mutex<Instant>> = OnceLock::new();
    {
        let mut last = LAST_CHECK
            .get_or_init(|| Mutex::new(Instant::now() - std::time::Duration::from_secs(10)))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if last.elapsed() < std::time::Duration::from_millis(500) {
            return;
        }
        *last = Instant::now();
    }

    if osm_ingest::has_active_jobs(model.selected_root.as_deref()) {
        return;
    }

    // Radius = viewport half-extent in miles, so the import always covers
    // the full visible area regardless of zoom level.
    let half_deg = local_terrain_scene::visual_half_extent_for_zoom(model.globe_view.local_zoom);
    let radius_miles = (half_deg * 69.0).clamp(8.0, 60.0);

    if let Some(focus) = model.terrain_focus_location() {
        queue_road_focus_import(model, focus, radius_miles, "terrain focus");
    }

    let center = model.globe_view.local_center;
    if model
        .terrain_focus_location()
        .map(|focus| (focus.lat - center.lat).abs() > 0.15 || (focus.lon - center.lon).abs() > 0.15)
        .unwrap_or(true)
    {
        queue_road_focus_import(model, center, radius_miles, "map viewport");
    }
}

pub(super) fn queue_road_focus_import(
    model: &mut AppModel,
    point: crate::model::GeoPoint,
    radius_miles: f32,
    label: &str,
) {
    match osm_ingest::queue_focus_roads_import(model.selected_root.as_deref(), point, radius_miles)
    {
        Ok(true) => {
            model.push_log(format!("Queued focused road import for the {label}."));
            model.osm_inventory =
                osm_ingest::OsmInventory::detect_from(model.selected_root.as_deref());
        }
        Ok(false) => {}
        Err(error) => {
            model.push_log(format!("Focused road import failed: {error}"));
        }
    }
}

pub(super) fn ensure_visible_water_layers(model: &mut AppModel, local_terrain_mode: bool) {
    if !local_terrain_mode || !model.show_water {
        return;
    }

    static LAST_WATER_CHECK: OnceLock<Mutex<Instant>> = OnceLock::new();
    {
        let mut last = LAST_WATER_CHECK
            .get_or_init(|| Mutex::new(Instant::now() - std::time::Duration::from_secs(10)))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if last.elapsed() < std::time::Duration::from_millis(500) {
            return;
        }
        *last = Instant::now();
    }

    if osm_ingest::has_active_jobs(model.selected_root.as_deref()) {
        return;
    }

    let half_deg = local_terrain_scene::visual_half_extent_for_zoom(model.globe_view.local_zoom);
    let radius_miles = (half_deg * 69.0 * 1.25).clamp(10.0, 150.0);

    if let Some(focus) = model.terrain_focus_location() {
        queue_water_focus_import(model, focus, radius_miles, "terrain focus");
    }

    let center = model.globe_view.local_center;
    if model
        .terrain_focus_location()
        .map(|focus| (focus.lat - center.lat).abs() > 0.15 || (focus.lon - center.lon).abs() > 0.15)
        .unwrap_or(true)
    {
        queue_water_focus_import(model, center, radius_miles, "map viewport");
    }
}

pub(super) fn queue_water_focus_import(
    model: &mut AppModel,
    point: crate::model::GeoPoint,
    radius_miles: f32,
    label: &str,
) {
    match osm_ingest::queue_focus_water_import(model.selected_root.as_deref(), point, radius_miles)
    {
        Ok(true) => {
            model.push_log(format!("Queued focused water import for the {label}."));
            model.osm_inventory =
                osm_ingest::OsmInventory::detect_from(model.selected_root.as_deref());
        }
        Ok(false) => {}
        Err(error) => {
            model.push_log(format!("Focused water import failed: {error}"));
        }
    }
}
