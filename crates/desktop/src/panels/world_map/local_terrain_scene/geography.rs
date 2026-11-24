use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;

use crate::model::{GeoPoint, GlobeViewState};

use super::super::contour_asset::{self, ContourPath};
use super::projection::{LocalProjectionParams, contains_point, local_projection_params};
use super::{LocalLayout, local_stroke_width_points, visual_half_extent_for_zoom};

#[derive(Clone, Copy)]
struct PolyBBox {
    min_lon: f32,
    min_lat: f32,
    max_lon: f32,
    max_lat: f32,
}

impl PolyBBox {
    fn from_points(points: &[GeoPoint]) -> Self {
        let mut min_lon = f32::INFINITY;
        let mut max_lon = f32::NEG_INFINITY;
        let mut min_lat = f32::INFINITY;
        let mut max_lat = f32::NEG_INFINITY;
        for p in points {
            min_lon = min_lon.min(p.lon);
            max_lon = max_lon.max(p.lon);
            min_lat = min_lat.min(p.lat);
            max_lat = max_lat.max(p.lat);
        }
        Self {
            min_lon,
            min_lat,
            max_lon,
            max_lat,
        }
    }

    #[inline]
    fn intersects(&self, bounds: [f32; 4]) -> bool {
        if self.max_lat < bounds[1] || self.min_lat > bounds[3] {
            return false;
        }
        let [w, _, e, _] = bounds;
        (self.max_lon >= w && self.min_lon <= e)
            || (self.max_lon + 360.0 >= w && self.min_lon + 360.0 <= e)
            || (self.max_lon - 360.0 >= w && self.min_lon - 360.0 <= e)
    }
}

struct CachedBBoxes {
    ptr: usize,
    len: usize,
    boxes: Vec<PolyBBox>,
}

fn with_bboxes<R>(
    cache: &RefCell<Option<CachedBBoxes>>,
    contours: &Arc<Vec<ContourPath>>,
    f: impl FnOnce(&[PolyBBox]) -> R,
) -> R {
    let ptr = Arc::as_ptr(contours) as usize;
    let len = contours.len();
    let mut slot = cache.borrow_mut();
    if slot.as_ref().is_none_or(|c| c.ptr != ptr || c.len != len) {
        let boxes = contours
            .iter()
            .map(|c| PolyBBox::from_points(&c.points))
            .collect();
        *slot = Some(CachedBBoxes { ptr, len, boxes });
    }
    f(&slot.as_ref().unwrap().boxes)
}

#[inline]
fn emit_clipped_runs(
    painter: &egui::Painter,
    params: &LocalProjectionParams,
    points: &[GeoPoint],
    elev_m: f32,
    bounds: [f32; 4],
    span: f32,
    stroke: egui::Stroke,
) {
    // Expand bounds by 10% so segments crossing the viewport edge connect cleanly.
    let lon_pad = ((bounds[2] - bounds[0]) * 0.10).max(0.5);
    let lat_pad = ((bounds[3] - bounds[1]) * 0.10).max(0.5);
    let padded = [
        bounds[0] - lon_pad,
        (bounds[1] - lat_pad).max(-90.0),
        bounds[2] + lon_pad,
        (bounds[3] + lat_pad).min(90.0),
    ];

    let mut run: Vec<egui::Pos2> = Vec::new();
    let mut prev_pt: Option<GeoPoint> = None;
    let mut prev_in = false;

    for &pt in points {
        let inside = contains_point(padded, pt);
        if inside {
            if !prev_in && let Some(prev) = prev_pt {
                let (x, y, _) = params.project_logical(prev, elev_m);
                if x.is_finite() && y.is_finite() && x.abs() < span && y.abs() < span {
                    run.push(egui::pos2(x, y));
                }
            }
            let (x, y, _) = params.project_logical(pt, elev_m);
            if x.is_finite() && y.is_finite() && x.abs() < span && y.abs() < span {
                run.push(egui::pos2(x, y));
            } else if run.len() >= 2 {
                painter.add(egui::Shape::line(std::mem::take(&mut run), stroke));
            } else {
                run.clear();
            }
        } else if prev_in {
            let (x, y, _) = params.project_logical(pt, elev_m);
            if x.is_finite() && y.is_finite() && x.abs() < span && y.abs() < span {
                run.push(egui::pos2(x, y));
            }
            if run.len() >= 2 {
                painter.add(egui::Shape::line(std::mem::take(&mut run), stroke));
            } else {
                run.clear();
            }
        }
        prev_pt = Some(pt);
        prev_in = inside;
    }
    if run.len() >= 2 {
        painter.add(egui::Shape::line(run, stroke));
    }
}

pub(super) fn draw_bathymetry_local(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    render_zoom: f32,
    selected_root: Option<&Path>,
    contour_stroke_scale: f32,
) {
    let pixels_per_point = painter.ctx().pixels_per_point();
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

    let params = local_projection_params(layout, view, focus, extent_x_km, extent_y_km);
    let bounds = params
        .ground_bounds(painter.clip_rect().expand(8.))
        .unwrap_or([-180., -90., 180., 90.]);
    let span = layout.width.max(layout.height) * 8.0;

    let _ = render_zoom; // used by caller for LOD selection via bathy_zoom
    const BATHY_ELEV_OFFSET: f32 = -5.0; // project just below sea level

    thread_local! {
        static BATHY_BBOXES: RefCell<Option<CachedBBoxes>> = const { RefCell::new(None) };
    }

    BATHY_BBOXES.with(|cache| {
        with_bboxes(cache, &bathy, |boxes| {
            for (contour, bbox) in bathy.iter().zip(boxes.iter()) {
                if !bbox.intersects(bounds) {
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
                let width = local_stroke_width_points(
                    (if major { 1.2 } else { 0.7 }) * contour_stroke_scale,
                    pixels_per_point,
                );
                let stroke = egui::Stroke::new(width, color);

                emit_clipped_runs(
                    painter,
                    &params,
                    &contour.points,
                    BATHY_ELEV_OFFSET,
                    bounds,
                    span,
                    stroke,
                );
            }
        });
    });
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
    contour_stroke_scale: f32,
) {
    let pixels_per_point = painter.ctx().pixels_per_point();
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * focus.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let params = local_projection_params(layout, view, focus, extent_x_km, extent_y_km);
    let bounds = params
        .ground_bounds(painter.clip_rect().expand(8.))
        .unwrap_or([-180., -90., 180., 90.]);
    let span = layout.width.max(layout.height) * 8.0;

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
        local_stroke_width_points(contour_stroke_scale, pixels_per_point),
        egui::Color32::from_rgba_premultiplied(220, 230, 255, 55),
    );

    thread_local! {
        static COAST_BBOXES: RefCell<Option<CachedBBoxes>> = const { RefCell::new(None) };
    }

    COAST_BBOXES.with(|cache| {
        with_bboxes(cache, &coastlines, |boxes| {
            for (coast, bbox) in coastlines.iter().zip(boxes.iter()) {
                if !bbox.intersects(bounds) {
                    continue;
                }
                emit_clipped_runs(
                    painter,
                    &params,
                    &coast.points,
                    COAST_ELEV,
                    bounds,
                    span,
                    stroke,
                );
            }
        });
    });
}
