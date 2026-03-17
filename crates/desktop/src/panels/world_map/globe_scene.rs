use crate::model::{AppModel, EventRecord, GeoPoint, GlobeViewState};
use crate::theme;

use super::camera::{self, GlobeLod};
use super::contour_asset;
use super::terrain_field;

pub struct GlobeScene {
    pub event_markers: Vec<(String, egui::Pos2)>,
    pub camera_markers: Vec<(String, egui::Pos2)>,
}

struct GlobeLayout {
    center: egui::Pos2,
    radius: f32,
    focal_length: f32,
    camera_distance: f32,
}

#[derive(Clone, Copy)]
struct ProjectedPoint {
    pos: egui::Pos2,
    depth: f32,
    front_facing: bool,
}

pub fn paint(painter: &egui::Painter, rect: egui::Rect, model: &AppModel, time: f64) -> GlobeScene {
    painter.rect_filled(rect, 12.0, theme::canvas_background());

    let lod = camera::lod(&model.globe_view);
    let layout = globe_layout(rect, &model.globe_view);
    let selected_root = model.selected_root.as_deref();

    draw_backdrop(painter, rect, &layout);
    draw_hud_frame(painter, rect);
    draw_wireframe(painter, &layout, &model.globe_view, &lod);
    draw_global_coastlines(painter, &layout, &model.globe_view, selected_root);
    draw_global_topo(painter, &layout, &model.globe_view, selected_root);

    draw_srtm_on_globe(painter, &layout, &model.globe_view, &lod, selected_root);

    let selected_event_id = model.selected_event_id.as_deref();
    let selected_camera_id = model.selected_camera_id.as_deref();
    let nearby = model.nearby_cameras(250.0);

    let event_markers: Vec<_> = model
        .events
        .iter()
        .filter_map(|event| {
            project_geo(
                &layout,
                &model.globe_view,
                event.location,
                lod.altitude_scale * 0.7,
            )
            .map(|projected| {
                draw_event_marker(
                    painter,
                    projected,
                    event,
                    selected_event_id == Some(event.id.as_str()),
                    time,
                );
                (event.id.clone(), projected.pos)
            })
        })
        .collect();

    let camera_markers: Vec<_> = nearby
        .iter()
        .filter_map(|camera| {
            project_geo(
                &layout,
                &model.globe_view,
                camera.location,
                lod.altitude_scale * 0.35,
            )
            .map(|projected| {
                let is_selected = selected_camera_id == Some(camera.id.as_str());
                draw_camera_marker(painter, projected, is_selected);
                (camera.id.clone(), projected.pos)
            })
        })
        .collect();

    if let Some((_, event_marker)) = event_markers
        .iter()
        .find(|(event_id, _)| selected_event_id == Some(event_id.as_str()))
    {
        draw_camera_links(painter, *event_marker, &camera_markers);
    }

    draw_legend(painter, rect, &layout, &model.globe_view, &lod);

    GlobeScene {
        event_markers,
        camera_markers,
    }
}

fn globe_layout(rect: egui::Rect, view: &GlobeViewState) -> GlobeLayout {
    // zoom_t: 0 at minimum globe zoom, 1 at the local-terrain transition threshold
    let zoom_t = ((view.zoom.ln() - 0.6f32.ln())
        / (super::local_terrain_scene::LOCAL_MODE_MIN_ZOOM.ln() - 0.6f32.ln()))
    .clamp(0.0, 1.0);
    // Globe grows to nearly fill the panel as you zoom in, giving continuous spatial context
    // before the terrain view takes over. Base is sized to leave room for the HUD frame.
    let base_radius = (rect.width() * 0.21).min(rect.height() * 0.30);
    // Growth factor 1.0 → globe reaches 2× base radius by the local-terrain threshold,
    // nearly filling the panel so the transition feels like landing rather than jumping.
    let radius = base_radius * (1.0 + zoom_t * 1.0);
    GlobeLayout {
        center: egui::pos2(
            rect.center().x + rect.width() * 0.04,
            rect.center().y + rect.height() * 0.01,
        ),
        radius,
        focal_length: 2.05 + zoom_t * 0.3,
        camera_distance: 3.15 - zoom_t * 1.05,
    }
}

fn draw_backdrop(painter: &egui::Painter, rect: egui::Rect, layout: &GlobeLayout) {
    painter.circle_filled(
        layout.center,
        layout.radius * 0.998,
        egui::Color32::from_rgba_premultiplied(2, 6, 10, 252),
    );

    painter.circle_stroke(
        layout.center,
        layout.radius,
        egui::Stroke::new(1.25, theme::wireframe_color().gamma_multiply(0.75)),
    );

    painter.rect_stroke(
        rect.shrink(6.0),
        12.0,
        egui::Stroke::new(0.7, theme::topo_color().gamma_multiply(0.45)),
        egui::StrokeKind::Outside,
    );
}

fn draw_hud_frame(painter: &egui::Painter, rect: egui::Rect) {
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

fn draw_wireframe(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    lod: &GlobeLod,
) {
    if view.zoom >= super::local_terrain_scene::LOCAL_MODE_MIN_ZOOM {
        return;
    }

    for lat in (-80..=80).step_by(lod.lat_line_step) {
        let path: Vec<_> = (-180..=180)
            .step_by(lod.sample_step)
            .map(|lon| GeoPoint {
                lat: lat as f32,
                lon: lon as f32,
            })
            .collect();
        draw_geo_path(
            painter,
            layout,
            view,
            &path,
            lod.altitude_scale,
            theme::contour_color(),
            lod.backface_alpha * 0.42,
        );
    }

    for lon in (-180..=180).step_by(lod.lon_line_step) {
        let path: Vec<_> = (-85..=85)
            .step_by(lod.sample_step)
            .map(|lat| GeoPoint {
                lat: lat as f32,
                lon: lon as f32,
            })
            .collect();
        draw_geo_path(
            painter,
            layout,
            view,
            &path,
            lod.altitude_scale,
            theme::grid_color(),
            lod.backface_alpha * 0.32,
        );
    }
}

fn draw_global_coastlines(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    selected_root: Option<&std::path::Path>,
) {
    let Some(coastlines) = contour_asset::load_global_coastlines(selected_root, view.zoom) else {
        return;
    };

    for coastline in coastlines.iter() {
        draw_geo_path(
            painter,
            layout,
            view,
            &coastline.points,
            0.022,
            egui::Color32::from_rgb(142, 234, 246),
            0.16,
        );
    }
}

fn draw_global_topo(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    selected_root: Option<&std::path::Path>,
) {
    // Crossfade: full opacity at zoom ≤ 3.0, fade to zero by zoom 5.0.
    // SRTM globe tiles fade in from 1.5→3.0, so there is overlap in the
    // 3–5× range where both layers contribute before SRTM dominates.
    let alpha = (1.0 - (view.zoom - 3.0) / 2.0).clamp(0.0, 1.0);
    if alpha <= 0.01 {
        return;
    }

    let Some(topo) = contour_asset::load_global_topo(selected_root, view.zoom) else {
        return;
    };

    for contour in topo.iter() {
        let major = (contour.elevation_m.round() as i32).rem_euclid(2_000) == 0;
        let color = if major {
            egui::Color32::from_rgb(210, 95, 45)
        } else {
            egui::Color32::from_rgb(115, 185, 210)
        };
        draw_geo_path(
            painter,
            layout,
            view,
            &contour.points,
            0.015,
            color.gamma_multiply(alpha),
            0.05 * alpha,
        );
    }
}

/// Draw SRTM focus-tile contours directly on the sphere surface.
/// Fades in from zoom 2.0 → 4.0, crossfading with the coarser global topo.
/// Because these go through `draw_geo_path` / `project_geo` they are
/// sphere-projected and rotate with the globe — no floating flat overlay.
fn draw_srtm_on_globe(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    lod: &GlobeLod,
    selected_root: Option<&std::path::Path>,
) {
    if view.zoom < 1.5 {
        return;
    }
    // Fade in over 1.5→3.0x.  Tiles are fixed-size (zoom_bucket=1, ~2.2°
    // half-extent) so they maintain constant apparent size on screen as the
    // globe grows rather than shrinking with each zoom step.
    let alpha = ((view.zoom - 1.5) / 1.5).clamp(0.0, 1.0);

    let Some(contours) = contour_asset::load_srtm_for_globe(
        selected_root,
        view.local_center,
        view.zoom,
    ) else {
        return;
    };

    for contour in contours.iter() {
        let major = (contour.elevation_m.round() as i32).rem_euclid(50) == 0;
        let color = if major {
            egui::Color32::from_rgb(244, 123, 61)
        } else {
            egui::Color32::from_rgb(121, 212, 236)
        };
        // Use the same altitude_scale as coastlines (0.022) so SRTM contours
        // sit on the sphere surface and don't parallax against the coastline layer.
        // lod.altitude_scale is designed for exaggerated local-terrain relief and
        // would push these contours visibly above the globe radius.
        draw_geo_path(
            painter,
            layout,
            view,
            &contour.points,
            0.020,
            color.gamma_multiply(alpha),
            0.08,
        );
    }
}

fn draw_real_contours(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    lod: &GlobeLod,
    contours: &[contour_asset::ContourPath],
) {
    for contour in contours {
        let emphasis = ((contour.elevation_m / 1000.0).abs() % 5.0) < 0.5;
        draw_geo_path(
            painter,
            layout,
            view,
            &contour.points,
            lod.altitude_scale,
            if emphasis {
                egui::Color32::from_rgb(244, 123, 61)
            } else {
                egui::Color32::from_rgb(198, 229, 236)
            },
            lod.backface_alpha * 0.1,
        );
    }
}

fn draw_event_marker(
    painter: &egui::Painter,
    marker: ProjectedPoint,
    event: &EventRecord,
    is_selected: bool,
    time: f64,
) {
    let radius = 4.8 + marker.depth * 1.8;
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
        radius + 2.2,
        egui::Stroke::new(0.9, theme::hot_color().gamma_multiply(0.75)),
    );
}

fn draw_camera_marker(painter: &egui::Painter, marker: ProjectedPoint, is_selected: bool) {
    let radius = 3.0 + marker.depth;
    let color = if is_selected {
        egui::Color32::from_rgb(215, 245, 252)
    } else {
        theme::camera_color()
    };

    painter.circle_filled(marker.pos, radius, color);
    if is_selected {
        painter.circle_stroke(marker.pos, radius + 3.2, egui::Stroke::new(1.1, color));
    }
}

fn draw_camera_links(
    painter: &egui::Painter,
    event_marker: egui::Pos2,
    camera_markers: &[(String, egui::Pos2)],
) {
    for (_, marker) in camera_markers {
        painter.line_segment(
            [event_marker, *marker],
            egui::Stroke::new(0.8, theme::camera_color().gamma_multiply(0.36)),
        );
    }
}

fn draw_legend(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    lod: &GlobeLod,
) {
    painter.text(
        egui::pos2(rect.left() + 24.0, rect.bottom() - 86.0),
        egui::Align2::LEFT_TOP,
        format!(
            "TACTICAL GLOBE\n3D ORBIT {}\nZOOM {:.2}x | LOD {}",
            if view.auto_spin { "AUTO" } else { "MANUAL" },
            view.zoom,
            lod.contour_layers
        ),
        egui::FontId::monospace(12.0),
        theme::text_muted(),
    );

    painter.text(
        egui::pos2(
            layout.center.x + layout.radius + 52.0,
            layout.center.y - 22.0,
        ),
        egui::Align2::LEFT_TOP,
        "RANGE GATE\n250 KM",
        egui::FontId::monospace(11.0),
        theme::hot_color(),
    );
}

fn draw_geo_path(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    path: &[GeoPoint],
    altitude_scale: f32,
    front_color: egui::Color32,
    backface_alpha: f32,
) {
    let mut front_segment = Vec::new();
    let mut back_segment = Vec::new();

    for point in path {
        if let Some(projected) = project_geo(layout, view, *point, altitude_scale) {
            if projected.front_facing {
                if back_segment.len() >= 2 {
                    painter.add(egui::Shape::line(
                        std::mem::take(&mut back_segment),
                        egui::Stroke::new(0.55, front_color.gamma_multiply(backface_alpha)),
                    ));
                }
                front_segment.push(projected.pos);
            } else {
                if front_segment.len() >= 2 {
                    painter.add(egui::Shape::line(
                        std::mem::take(&mut front_segment),
                        egui::Stroke::new(1.15, front_color.gamma_multiply(0.88)),
                    ));
                }
                back_segment.push(projected.pos);
            }
        } else {
            flush_segments(
                painter,
                &mut front_segment,
                &mut back_segment,
                front_color,
                backface_alpha,
            );
        }
    }

    flush_segments(
        painter,
        &mut front_segment,
        &mut back_segment,
        front_color,
        backface_alpha,
    );
}

fn flush_segments(
    painter: &egui::Painter,
    front_segment: &mut Vec<egui::Pos2>,
    back_segment: &mut Vec<egui::Pos2>,
    front_color: egui::Color32,
    backface_alpha: f32,
) {
    if front_segment.len() >= 2 {
        painter.add(egui::Shape::line(
            std::mem::take(front_segment),
            egui::Stroke::new(0.95, front_color.gamma_multiply(0.92)),
        ));
    } else {
        front_segment.clear();
    }

    if back_segment.len() >= 2 {
        painter.add(egui::Shape::line(
            std::mem::take(back_segment),
            egui::Stroke::new(0.4, front_color.gamma_multiply(backface_alpha)),
        ));
    } else {
        back_segment.clear();
    }
}

fn project_geo(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
    altitude_scale: f32,
) -> Option<ProjectedPoint> {
    let lat = point.lat.to_radians();
    let lon = point.lon.to_radians();
    let elevation_signal = terrain_field::elevation(point) / 1.6;
    let signed_elevation = elevation_signal.mul_add(2.0, -1.0);
    let elevation = signed_elevation * altitude_scale;
    let radius = (1.0 + elevation).max(0.82);

    let mut x = radius * lat.cos() * lon.cos();
    let mut y = radius * lat.sin();
    let mut z = radius * lat.cos() * lon.sin();

    let yaw_cos = view.yaw.cos();
    let yaw_sin = view.yaw.sin();
    let x_yaw = x * yaw_cos + z * yaw_sin;
    let z_yaw = -x * yaw_sin + z * yaw_cos;
    x = x_yaw;
    z = z_yaw;

    let pitch_cos = view.pitch.cos();
    let pitch_sin = view.pitch.sin();
    let y_pitch = y * pitch_cos - z * pitch_sin;
    let z_pitch = y * pitch_sin + z * pitch_cos;
    y = y_pitch;
    z = z_pitch;

    let depth = layout.camera_distance - z;
    if depth <= 0.05 {
        return None;
    }

    let perspective = (layout.radius * layout.focal_length) / depth;
    let pos = egui::pos2(
        layout.center.x - x * perspective,
        layout.center.y - y * perspective,
    );

    Some(ProjectedPoint {
        pos,
        depth: ((z + 1.0) * 0.5).clamp(0.0, 1.0),
        front_facing: z >= 0.0,
    })
}
