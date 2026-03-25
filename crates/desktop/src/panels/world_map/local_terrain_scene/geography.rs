use std::path::Path;

use crate::model::{GeoPoint, GlobeViewState};

use super::super::contour_asset;
use super::projection::project_local;
use super::{LocalLayout, visual_half_extent_for_zoom};

pub(super) fn draw_bathymetry_local(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    render_zoom: f32,
    selected_root: Option<&Path>,
) {
    // Use GEBCO bathymetry — same zoom/LOD approach as global coastline.
    let bathy_zoom = view.local_zoom.clamp(1.0, 8.0);
    let Some(bathy) =
        contour_asset::load_global_bathymetry(selected_root, bathy_zoom, painter.ctx().clone())
    else {
        return;
    };

    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * focus.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let margin = half_extent_deg * 1.5;
    let min_lat = focus.lat - margin;
    let max_lat = focus.lat + margin;
    let min_lon = focus.lon - margin;
    let max_lon = focus.lon + margin;

    let _ = render_zoom; // used by caller for LOD selection via bathy_zoom

    const BATHY_ELEV_OFFSET: f32 = -5.0; // project just below sea level

    for contour in bathy.iter() {
        let in_view = contour
            .points
            .iter()
            .any(|p| p.lat >= min_lat && p.lat <= max_lat && p.lon >= min_lon && p.lon <= max_lon);
        if !in_view {
            continue;
        }

        let depth_norm = (-contour.elevation_m / 11_000.0_f32).clamp(0.0, 1.0);
        let major = ((-contour.elevation_m.round() as i32) % 1_000) < 50;
        let base_a = if major { 0.50_f32 } else { 0.25_f32 };
        let a = (base_a * (0.4 + depth_norm * 0.6) * 255.0) as u8;
        let r = (18.0 * (1.0 - depth_norm * 0.8)) as u8;
        let g = (55.0 * (1.0 - depth_norm * 0.6)) as u8;
        let b = (130 + (60.0 * depth_norm) as u8).min(255);
        let color = egui::Color32::from_rgba_premultiplied(r, g, b, a);
        let width = if major { 1.2 } else { 0.7 };
        let stroke = egui::Stroke::new(width, color);

        let points: Vec<_> = contour
            .points
            .iter()
            .filter_map(|p| {
                project_local(
                    layout,
                    view,
                    focus,
                    *p,
                    BATHY_ELEV_OFFSET,
                    extent_x_km,
                    extent_y_km,
                )
                .map(|pp| pp.pos)
            })
            .collect();

        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}

/// Draw global coastlines projected into the local oblique view.
/// Filters to only the polyline segments that overlap the current viewport.
pub(super) fn draw_coastlines_local(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    _render_zoom: f32,
    selected_root: Option<&Path>,
) {
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * focus.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let margin = half_extent_deg * 1.5;
    let min_lat = focus.lat - margin;
    let max_lat = focus.lat + margin;
    let min_lon = focus.lon - margin;
    let max_lon = focus.lon + margin;

    // GEBCO-derived global coastline (450m resolution).
    // Single LOD in load_global_coastlines so this never reloads on zoom change.
    let Some(coastlines) =
        contour_asset::load_global_coastlines(selected_root, 1.0, painter.ctx().clone())
    else {
        return;
    };

    // Single thin white line — same visual weight as the topo contours.
    const COAST_ELEV: f32 = -3.0;
    let stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgba_premultiplied(220, 230, 255, 55),
    );

    for coast in coastlines.iter() {
        let in_view = coast
            .points
            .iter()
            .any(|p| p.lat >= min_lat && p.lat <= max_lat && p.lon >= min_lon && p.lon <= max_lon);
        if !in_view {
            continue;
        }
        let points: Vec<_> = coast
            .points
            .iter()
            .filter_map(|p| {
                project_local(
                    layout,
                    view,
                    focus,
                    *p,
                    COAST_ELEV,
                    extent_x_km,
                    extent_y_km,
                )
                .map(|pp| pp.pos)
            })
            .collect();
        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}
