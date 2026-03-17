use crate::model::{AppModel, EventRecord, GeoPoint, GlobeViewState, NearbyCamera};
use crate::terrain_assets;
use crate::theme;
use std::path::Path;

use super::contour_asset;
use super::globe_scene::GlobeScene;
use super::srtm_focus_cache;
use super::srtm_stream;

pub const LOCAL_TRANSITION_START_ZOOM: f32 = 4.0;
pub const LOCAL_MODE_MIN_ZOOM: f32 = 25.0;
const LOCAL_STREAM_RADIUS: i32 = 2;

struct LocalLayout {
    center: egui::Pos2,
    focus_center: egui::Pos2,
    width: f32,
    height: f32,
    horizontal_scale: f32,
}

#[derive(Clone, Copy)]
struct ProjectedLocalPoint {
    pos: egui::Pos2,
    depth: f32,
}

pub fn paint(painter: &egui::Painter, rect: egui::Rect, model: &AppModel, time: f64) -> GlobeScene {
    painter.rect_filled(rect, 12.0, theme::canvas_background());
    draw_frame(painter, rect);

    let layout = layout(rect);
    let Some(focus) = model.terrain_focus_location() else {
        draw_empty_state(painter, rect, "No terrain focus selected");
        return GlobeScene {
            event_markers: Vec::new(),
            camera_markers: Vec::new(),
        };
    };

    let viewport_center = model.globe_view.local_center;
    let render_zoom = local_render_zoom(model.globe_view.zoom);
    let contours = contour_asset::load_srtm_region_for_view(
        model.selected_root.as_deref(),
        focus,
        viewport_center,
        render_zoom,
        LOCAL_STREAM_RADIUS,
    );
    let cache_status = srtm_focus_cache::focus_contour_region_status(
        model.selected_root.as_deref(),
        viewport_center,
        render_zoom,
        LOCAL_STREAM_RADIUS,
    );

    let nearby = if model.focused_city().is_none() {
        model.nearby_cameras(250.0)
    } else {
        Vec::new()
    };

    if let Some(contours) = contours.as_deref() {
        draw_contour_stack(
            painter,
            &layout,
            &model.globe_view,
            viewport_center,
            render_zoom,
            contours,
            1.0,
        );
        let (event_markers, camera_markers) = if let Some(event) = model.selected_event() {
            draw_markers(
                painter,
                &layout,
                &model.globe_view,
                model.selected_root.as_deref(),
                viewport_center,
                render_zoom,
                event,
                &nearby,
                model.selected_event_id.as_deref(),
                model.selected_camera_id.as_deref(),
                time,
            )
        } else {
            (Vec::new(), Vec::new())
        };
        draw_camera_links(
            painter,
            event_markers.first().map(|(_, pos)| *pos),
            &camera_markers,
        );
        draw_legend(painter, rect, "LOCAL EVENT TERRAIN", render_zoom);
        if let Some(status) = cache_status {
            draw_cache_progress(painter, rect, status);
        }

        GlobeScene {
            event_markers,
            camera_markers,
        }
    } else {
        draw_empty_state(painter, rect, "Generating local terrain cache...");
        draw_legend(painter, rect, "LOCAL EVENT TERRAIN", render_zoom);
        if let Some(status) = cache_status {
            draw_cache_progress(painter, rect, status);
        }
        GlobeScene {
            event_markers: Vec::new(),
            camera_markers: Vec::new(),
        }
    }
}

pub fn paint_transition_overlay(
    painter: &egui::Painter,
    rect: egui::Rect,
    model: &AppModel,
    progress: f32,
) {
    if progress <= 0.0 {
        return;
    }

    let Some(focus) = model.terrain_focus_location() else {
        return;
    };

    let viewport_center = model.globe_view.local_center;
    let render_zoom = local_render_zoom(model.globe_view.zoom);
    let Some(contours) = contour_asset::load_srtm_region_for_view(
        model.selected_root.as_deref(),
        focus,
        viewport_center,
        render_zoom,
        LOCAL_STREAM_RADIUS,
    ) else {
        return;
    };

    let layout = transition_layout(rect, progress);
    draw_contour_stack(
        painter,
        &layout,
        &model.globe_view,
        viewport_center,
        render_zoom,
        contours.as_ref(),
        progress,
    );
}

pub fn is_active(model: &AppModel) -> bool {
    model.globe_view.zoom >= LOCAL_MODE_MIN_ZOOM
        && model.terrain_focus_location().is_some()
        && terrain_assets::find_srtm_root(model.selected_root.as_deref()).is_some()
}

pub fn transition_progress(zoom: f32) -> f32 {
    ((zoom - LOCAL_TRANSITION_START_ZOOM) / (LOCAL_MODE_MIN_ZOOM - LOCAL_TRANSITION_START_ZOOM))
        .clamp(0.0, 1.0)
}

pub fn has_pending_cache(model: &AppModel) -> bool {
    let Some(_) = model.terrain_focus_location() else {
        return false;
    };

    let render_zoom = local_render_zoom(model.globe_view.zoom);
    srtm_focus_cache::focus_contour_region_status(
        model.selected_root.as_deref(),
        model.globe_view.local_center,
        render_zoom,
        LOCAL_STREAM_RADIUS,
    )
    .map(|status| status.pending_assets > 0 && status.ready_assets < status.total_assets)
    .unwrap_or(false)
}

pub fn local_render_zoom(view_zoom: f32) -> f32 {
    view_zoom.clamp(LOCAL_TRANSITION_START_ZOOM, 20.0)
}

pub fn visual_half_extent_for_zoom(view_zoom: f32) -> f32 {
    const KNOTS: &[(f32, f32)] = &[
        (LOCAL_TRANSITION_START_ZOOM, 1.55), // 4.0 → ~173 km half-span (transition start)
        (5.5, 0.90),                         // 5.5 → ~100 km
        (7.0, 0.55),                         // 7.0 → ~61 km (local terrain fully active)
        (9.5, 0.31),                         // 9.5 → ~35 km
        (12.0, 0.17),                        // 12.0 → ~19 km
        (16.0, 0.09),                        // 16.0 → ~10 km
        (20.0, 0.045),                       // 20.0 → ~5 km
    ];

    let zoom = view_zoom.clamp(LOCAL_TRANSITION_START_ZOOM, 20.0);
    for window in KNOTS.windows(2) {
        let (start_zoom, start_extent) = window[0];
        let (end_zoom, end_extent) = window[1];
        if zoom <= end_zoom {
            let t = ((zoom - start_zoom) / (end_zoom - start_zoom)).clamp(0.0, 1.0);
            let start_log = start_extent.ln();
            let end_log = end_extent.ln();
            return egui::lerp(start_log..=end_log, t).exp();
        }
    }

    KNOTS.last().map(|(_, extent)| *extent).unwrap_or(0.17)
}

fn layout(rect: egui::Rect) -> LocalLayout {
    let width = rect.width() * 0.82;
    let height = rect.height() * 0.74;
    LocalLayout {
        center: rect.center(),
        focus_center: egui::pos2(
            rect.center().x + rect.width() * 0.02,
            rect.center().y + 12.0,
        ),
        width,
        height,
        horizontal_scale: rect.width() * 0.31,
    }
}

fn transition_layout(rect: egui::Rect, progress: f32) -> LocalLayout {
    let progress = progress.clamp(0.0, 1.0);
    let target = layout(rect);
    let scale = egui::lerp(0.52..=1.0, progress);
    let vertical_origin = egui::lerp(
        (rect.center().y + rect.height() * 0.1)..=(target.focus_center.y),
        progress,
    );

    LocalLayout {
        center: target.center,
        focus_center: egui::pos2(target.focus_center.x, vertical_origin),
        width: target.width * scale,
        height: target.height * scale,
        horizontal_scale: target.horizontal_scale * scale,
    }
}

fn draw_frame(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_stroke(
        rect.shrink(6.0),
        12.0,
        egui::Stroke::new(0.7, theme::topo_color().gamma_multiply(0.45)),
        egui::StrokeKind::Outside,
    );

    for &(x, y, x_dir, y_dir) in &[
        (rect.left() + 18.0, rect.top() + 18.0, 28.0, 16.0),
        (rect.right() - 18.0, rect.top() + 18.0, -28.0, 16.0),
        (rect.left() + 18.0, rect.bottom() - 18.0, 28.0, -16.0),
        (rect.right() - 18.0, rect.bottom() - 18.0, -28.0, -16.0),
    ] {
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2(x + x_dir, y)],
            egui::Stroke::new(1.0, theme::topo_color()),
        );
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2(x, y + y_dir)],
            egui::Stroke::new(1.0, theme::topo_color()),
        );
    }
}

fn draw_contour_stack(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    _render_zoom: f32,
    contours: &[contour_asset::ContourPath],
    alpha: f32,
) {
    let half_extent_deg = visual_half_extent_for_zoom(view.zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * focus.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let mut ordered: Vec<_> = contours.iter().collect();
    ordered.sort_by(|left, right| left.elevation_m.total_cmp(&right.elevation_m));

    for contour in ordered {
        let points: Vec<_> = contour
            .points
            .iter()
            .filter_map(|point| {
                project_local(
                    layout,
                    view,
                    focus,
                    *point,
                    contour.elevation_m,
                    extent_x_km,
                    extent_y_km,
                )
                .map(|projected| projected.pos)
            })
            .collect();

        if points.len() < 2 {
            continue;
        }

        let major = (contour.elevation_m.round() as i32).rem_euclid(50) == 0;
        let stroke = egui::Stroke::new(
            if major { 1.35 } else { 0.7 } * (0.72 + alpha * 0.28),
            if major {
                egui::Color32::from_rgb(244, 123, 61)
            } else {
                egui::Color32::from_rgb(121, 212, 236)
            }
            .gamma_multiply((if major { 1.0 } else { 0.78 }) * alpha),
        );

        painter.add(egui::Shape::line(points, stroke));
    }
}

fn draw_markers(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    event: &EventRecord,
    nearby: &[NearbyCamera],
    selected_event_id: Option<&str>,
    selected_camera_id: Option<&str>,
    time: f64,
) -> (Vec<(String, egui::Pos2)>, Vec<(String, egui::Pos2)>) {
    let half_extent_deg = visual_half_extent_for_zoom(view.zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let event_marker = project_local(
        layout,
        view,
        viewport_center,
        event.location,
        marker_elevation_m(selected_root, event.location),
        extent_x_km,
        extent_y_km,
    );
    if let Some(event_marker) = event_marker {
        draw_event_marker(
            painter,
            event_marker,
            event,
            selected_event_id == Some(event.id.as_str()),
            time,
        );
    }

    let camera_markers = nearby
        .iter()
        .filter_map(|camera| {
            project_local(
                layout,
                view,
                viewport_center,
                camera.location,
                marker_elevation_m(selected_root, camera.location),
                extent_x_km,
                extent_y_km,
            )
            .map(|projected| {
                draw_camera_marker(
                    painter,
                    projected,
                    selected_camera_id == Some(camera.id.as_str()),
                );
                (camera.id.clone(), projected.pos)
            })
        })
        .collect();

    (
        event_marker
            .map(|marker| vec![(event.id.clone(), marker.pos)])
            .unwrap_or_default(),
        camera_markers,
    )
}

fn draw_camera_links(
    painter: &egui::Painter,
    event_marker: Option<egui::Pos2>,
    camera_markers: &[(String, egui::Pos2)],
) {
    let Some(event_marker) = event_marker else {
        return;
    };

    for (_, marker) in camera_markers {
        painter.line_segment(
            [event_marker, *marker],
            egui::Stroke::new(0.75, theme::camera_color().gamma_multiply(0.32)),
        );
    }
}

fn draw_event_marker(
    painter: &egui::Painter,
    marker: ProjectedLocalPoint,
    event: &EventRecord,
    is_selected: bool,
    time: f64,
) {
    let radius = 5.1 + marker.depth * 1.8;
    if is_selected {
        let pulse = radius + 4.0 + ((time as f32 * 2.5).sin() + 1.0) * 2.4;
        painter.circle_stroke(
            marker.pos,
            pulse,
            egui::Stroke::new(
                1.3,
                egui::Color32::from_rgba_premultiplied(255, 241, 212, 170),
            ),
        );
    }

    painter.circle_filled(marker.pos, radius, event.severity.color());
    painter.circle_stroke(
        marker.pos,
        radius + 2.1,
        egui::Stroke::new(1.0, theme::hot_color().gamma_multiply(0.8)),
    );
}

fn draw_camera_marker(painter: &egui::Painter, marker: ProjectedLocalPoint, is_selected: bool) {
    let radius = 3.4 + marker.depth;
    let color = if is_selected {
        egui::Color32::from_rgb(215, 245, 252)
    } else {
        theme::camera_color()
    };

    painter.circle_filled(marker.pos, radius, color);
    if is_selected {
        painter.circle_stroke(marker.pos, radius + 3.0, egui::Stroke::new(1.1, color));
    }
}

fn draw_legend(painter: &egui::Painter, rect: egui::Rect, title: &str, render_zoom: f32) {
    let interval_m = srtm_focus_cache::contour_interval_for_zoom(render_zoom);
    let half_extent_km = visual_half_extent_for_zoom(render_zoom) * 111.32;
    painter.text(
        egui::pos2(rect.left() + 24.0, rect.bottom() - 86.0),
        egui::Align2::LEFT_TOP,
        format!(
            "{title}\nFIXED OBLIQUE CAMERA\n{interval_m}M CONTOURS · {half_extent_km:.0}KM HALF-SPAN"
        ),
        egui::FontId::monospace(12.0),
        theme::text_muted(),
    );
}

fn draw_cache_progress(
    painter: &egui::Painter,
    rect: egui::Rect,
    status: srtm_focus_cache::FocusContourRegionStatus,
) {
    if status.total_assets == 0 || status.ready_assets >= status.total_assets {
        return;
    }

    let frame_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 232.0, rect.bottom() - 88.0),
        egui::vec2(184.0, 36.0),
    );
    let bar_rect = egui::Rect::from_min_size(
        frame_rect.left_bottom() + egui::vec2(0.0, -12.0),
        egui::vec2(frame_rect.width(), 8.0),
    );
    let progress = (status.ready_assets as f32 / status.total_assets as f32).clamp(0.0, 1.0);

    painter.rect_filled(
        frame_rect,
        6.0,
        egui::Color32::from_rgba_premultiplied(7, 18, 24, 208),
    );
    painter.rect_stroke(
        frame_rect,
        6.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(24, 63, 79)),
        egui::StrokeKind::Outside,
    );
    painter.text(
        frame_rect.left_top() + egui::vec2(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        format!(
            "CACHE {} / {}  ·  {} PENDING",
            status.ready_assets, status.total_assets, status.pending_assets
        ),
        egui::FontId::monospace(11.0),
        theme::text_muted(),
    );
    painter.rect_filled(
        bar_rect,
        4.0,
        egui::Color32::from_rgba_premultiplied(15, 40, 49, 230),
    );
    if progress > 0.0 {
        let fill_rect = egui::Rect::from_min_max(
            bar_rect.min,
            egui::pos2(
                bar_rect.left() + bar_rect.width() * progress,
                bar_rect.bottom(),
            ),
        );
        painter.rect_filled(fill_rect, 4.0, theme::topo_color());
    }
}

fn draw_empty_state(painter: &egui::Painter, rect: egui::Rect, label: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(18.0),
        theme::text_muted(),
    );
}

fn marker_elevation_m(selected_root: Option<&Path>, point: GeoPoint) -> f32 {
    let terrain_elevation_m = srtm_stream::sample_elevation_m(selected_root, point).unwrap_or(0.0);
    terrain_elevation_m + 18.0
}

fn project_local(
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    point: GeoPoint,
    elevation_m: f32,
    extent_x_km: f32,
    extent_y_km: f32,
) -> Option<ProjectedLocalPoint> {
    let x_km = (point.lon - focus.lon) * 111.32 * focus.lat.to_radians().cos().abs().max(0.2);
    let y_km = (point.lat - focus.lat) * 111.32;

    let x = x_km / extent_x_km;
    let y = y_km / extent_y_km;
    let z = elevation_m / 1000.0;

    let yaw_cos = view.local_yaw.cos();
    let yaw_sin = view.local_yaw.sin();
    let x_yaw = x * yaw_cos - y * yaw_sin;
    let y_yaw = x * yaw_sin + y * yaw_cos;

    let pitch_cos = view.local_pitch.cos();
    let pitch_sin = view.local_pitch.sin();
    let ground_y_pitch = y_yaw * pitch_cos;
    let ground_z_pitch = y_yaw * pitch_sin;
    let elevation_y_offset = z * pitch_sin;
    let elevation_z_offset = z * pitch_cos;
    let z_pitch = ground_z_pitch + elevation_z_offset;

    let pos = egui::pos2(
        layout.focus_center.x + x_yaw * layout.horizontal_scale,
        layout.focus_center.y + ground_y_pitch * layout.height * 0.55
            - ground_z_pitch * 48.0
            - elevation_y_offset * view.local_layer_spread * 56.0
            - elevation_z_offset * view.local_layer_spread * 24.0,
    );

    (pos.x >= layout.center.x - layout.width * 0.58
        && pos.x <= layout.center.x + layout.width * 0.58
        && pos.y >= layout.center.y - layout.height * 0.62
        && pos.y <= layout.center.y + layout.height * 0.58)
        .then_some(ProjectedLocalPoint {
            pos,
            depth: (1.0 + z_pitch).clamp(0.0, 1.0),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_projection_expands_paris_contour_stack() {
        let model = AppModel::seed_demo();
        let event = model.selected_event().expect("selected event");
        let render_zoom = 6.0;
        let Some(contours) = (0..20).find_map(|_| {
            let contours = contour_asset::load_srtm_for_focus(
                model.selected_root.as_deref(),
                event.location,
                render_zoom,
            );
            if contours.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            contours
        }) else {
            return;
        };
        let layout = layout(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200.0, 900.0),
        ));
        let half_extent_deg = srtm_focus_cache::half_extent_for_zoom(render_zoom);
        let km_per_deg_lat = 111.32f32;
        let km_per_deg_lon = km_per_deg_lat * event.location.lat.to_radians().cos().abs().max(0.2);
        let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
        let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

        let points: Vec<_> = contours
            .iter()
            .flat_map(|contour| {
                contour
                    .points
                    .iter()
                    .map(move |point| (*point, contour.elevation_m))
            })
            .filter_map(|(point, elevation_m)| {
                project_local(
                    &layout,
                    &model.globe_view,
                    event.location,
                    point,
                    elevation_m,
                    extent_x_km,
                    extent_y_km,
                )
            })
            .collect();

        let min_x = points
            .iter()
            .map(|point| point.pos.x)
            .fold(f32::INFINITY, f32::min);
        let max_x = points
            .iter()
            .map(|point| point.pos.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = points
            .iter()
            .map(|point| point.pos.y)
            .fold(f32::INFINITY, f32::min);
        let max_y = points
            .iter()
            .map(|point| point.pos.y)
            .fold(f32::NEG_INFINITY, f32::max);

        assert!(!points.is_empty());
        assert!(max_x - min_x > 180.0);
        assert!(max_y - min_y > 140.0);
    }
}
