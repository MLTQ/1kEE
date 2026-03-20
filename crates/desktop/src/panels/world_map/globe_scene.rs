use crate::model::{AppModel, EventRecord, GeoPoint, GlobeViewState};
use crate::theme;

use super::camera::{self, GlobeLod};
use super::contour_asset;
use super::globe_pass;
use super::terrain_field;

pub struct GlobeScene {
    pub event_markers: Vec<(String, egui::Pos2)>,
    pub camera_markers: Vec<(String, egui::Pos2)>,
    /// Terrain elevation (metres) at the beam contact point, if available.
    pub beam_elevation_m: Option<f32>,
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

    // ── GPU globe backdrop (terrain shading + graticule) ───────────────────
    // Replaces the CPU draw_backdrop (flat circle) and draw_graticule (polylines).
    let ppp = painter.ctx().pixels_per_point();
    let show_grat = model.show_graticule && !model.globe_view.local_mode;
    painter.add(
        globe_pass::GlobeCallback::new(
            layout.center,
            layout.radius,
            layout.focal_length,
            layout.camera_distance,
            model.globe_view.yaw,
            model.globe_view.pitch,
            ppp,
            show_grat,
            theme::scene_backdrop(),
            theme::topo_color(),
            theme::wireframe_color(),
            theme::grid_color(),
            theme::hot_color(),
        )
        .into_paint_callback(rect),
    );

    // Outer panel rect stroke (was part of draw_backdrop)
    painter.rect_stroke(
        rect.shrink(6.0),
        12.0,
        egui::Stroke::new(0.7, theme::topo_color().gamma_multiply(0.45)),
        egui::StrokeKind::Outside,
    );

    if !model.cinematic_mode && model.show_reticle {
        draw_hud_frame(painter, rect);
    }
    if model.show_coastlines {
        draw_global_coastlines(painter, &layout, &model.globe_view, selected_root);
    }
    draw_global_topo(painter, &layout, &model.globe_view, selected_root);

    draw_srtm_on_globe(painter, &layout, &model.globe_view, &lod, selected_root);
    if !model.cinematic_mode && model.show_reticle {
        draw_zoom_crosshair(painter, &layout, &model.globe_view, time);
    }

    let selected_event_id = model.selected_event_id.as_deref();
    let selected_camera_id = model.selected_camera_id.as_deref();
    let nearby = model.nearby_cameras(250.0);

    let event_markers: Vec<_> = if model.cinematic_mode || !model.show_event_markers {
        Vec::new()
    } else {
        model
            .events
            .iter()
            .filter_map(|event| {
                let base = project_geo(
                    &layout,
                    &model.globe_view,
                    event.location,
                    lod.altitude_scale * 0.7,
                )?;
                // Beam tip: project the same geographic point at a higher
                // radius so that 3-D perspective foreshortening is correct.
                // When the event faces the camera, base and tip project to
                // almost the same screen position (tiny beam).  When the event
                // is on the limb, the tip projects far from the base (full
                // beam).  This eliminates the "spinning" artefact caused by
                // computing the direction in screen space.
                let extra_r = (82.0 / layout.radius).clamp(0.037, 0.135);
                let tip = project_geo_elevated(
                    &layout,
                    &model.globe_view,
                    event.location,
                    lod.altitude_scale * 0.7,
                    extra_r,
                )
                .map(|p| p.pos)
                .unwrap_or(base.pos); // fallback: zero-length beam
                draw_event_marker(
                    painter,
                    base,
                    tip,
                    event,
                    selected_event_id == Some(event.id.as_str()),
                    time,
                );
                Some((event.id.clone(), base.pos))
            })
            .collect()
    };

    let camera_markers: Vec<_> = if model.cinematic_mode {
        Vec::new()
    } else {
        nearby
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
            .collect()
    };

    if !model.cinematic_mode {
        if let Some((_, event_marker)) = event_markers
            .iter()
            .find(|(event_id, _)| selected_event_id == Some(event_id.as_str()))
        {
            draw_camera_links(painter, *event_marker, &camera_markers);
        }
        draw_legend(painter, rect, &layout, &model.globe_view, &lod);
    }

    GlobeScene {
        event_markers,
        camera_markers,
        beam_elevation_m: None,
    }
}

fn globe_layout(rect: egui::Rect, view: &GlobeViewState) -> GlobeLayout {
    // zoom_t: 0 at zoom=0.6, 1 at zoom=50.  Logarithmic so each scroll notch
    // gives equal perceived zoom step.
    let zoom_t = ((view.zoom.ln() - 0.6f32.ln()) / (50.0f32.ln() - 0.6f32.ln())).clamp(0.0, 1.0);
    let base_radius = (rect.width() * 0.21).min(rect.height() * 0.30);
    // At zoom_t=0 the globe is a small sphere; at zoom_t=1 it is 9× larger,
    // filling and greatly exceeding the viewport so only a country-scale
    // surface patch is visible.
    let radius = base_radius * (1.0 + zoom_t * 8.0);
    GlobeLayout {
        center: egui::pos2(
            rect.center().x + rect.width() * 0.04,
            rect.center().y + rect.height() * 0.01,
        ),
        radius,
        // Narrower FOV (higher focal_length) as we zoom in for a flatter,
        // more map-like perspective at high zoom.
        focal_length: 2.05 + zoom_t * 1.0,
        // Camera moves closer to the sphere surface at high zoom.
        // Keep at least 2.0 so the front pole stays visible (depth > 0).
        camera_distance: 3.15 - zoom_t * 1.15,
    }
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
            theme::contour_color().gamma_multiply(1.2),
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
            theme::hot_color()
        } else {
            theme::contour_color()
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

    let Some(contours) =
        contour_asset::load_srtm_for_globe(selected_root, view.local_center, view.zoom)
    else {
        return;
    };

    for contour in contours.iter() {
        let major = (contour.elevation_m.round() as i32).rem_euclid(50) == 0;
        let color = if major {
            theme::hot_color()
        } else {
            theme::contour_color()
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

/// Red glowing crosshair pinned to `view.local_center` — the point the camera
/// is centred on and will zoom into when transitioning to local terrain mode.
/// Provides spatial context for where you are on the globe surface.
fn draw_zoom_crosshair(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    time: f64,
) {
    // Only visible once the user has zoomed in enough to care about local terrain
    if view.zoom < 1.5 {
        return;
    }
    let alpha = ((view.zoom - 1.5) / 1.5).clamp(0.0, 1.0);

    let Some(projected) = project_geo(layout, view, view.local_center, 0.025) else {
        return;
    };

    // Cherry red — distinct from the orange "hot" palette used elsewhere
    let cherry = egui::Color32::from_rgb(210, 18, 50);
    let pos = projected.pos;
    let ring_r: f32 = 9.0;
    let gap: f32 = 3.5;
    let arm_len: f32 = 8.0;

    // Outer pulsing bloom ring
    let pulse = (time as f32 * 1.8).sin() * 0.5 + 0.5;
    let bloom_r = ring_r + 5.0 + pulse * 3.5;
    painter.circle_stroke(
        pos,
        bloom_r,
        egui::Stroke::new(
            6.0,
            cherry.gamma_multiply(alpha * 0.07 * (0.6 + pulse * 0.4)),
        ),
    );

    // Secondary soft halo
    painter.circle_stroke(
        pos,
        ring_r + 3.0,
        egui::Stroke::new(3.5, cherry.gamma_multiply(alpha * 0.18)),
    );

    // Crisp main ring
    painter.circle_stroke(
        pos,
        ring_r,
        egui::Stroke::new(1.3, cherry.gamma_multiply(alpha * 0.92)),
    );

    // Centre dot
    painter.circle_filled(pos, 2.0, cherry.gamma_multiply(alpha));

    // Four tick arms extending outward from the ring with a small gap
    let inner = ring_r + gap;
    let outer = ring_r + gap + arm_len;
    for &(dx, dy) in &[(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        painter.line_segment(
            [
                egui::pos2(pos.x + dx * inner, pos.y + dy * inner),
                egui::pos2(pos.x + dx * outer, pos.y + dy * outer),
            ],
            egui::Stroke::new(1.3, cherry.gamma_multiply(alpha * 0.85)),
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
            if emphasis { theme::hot_color() } else { theme::contour_color() },
            lod.backface_alpha * 0.1,
        );
    }
}

/// Draw a Factal event as a glowing surface-normal laser beam.
/// `base` is the ground-strike projected point; `tip` is the 3-D-projected
/// beam tip (not a screen-space offset, so perspective foreshortening is
/// correct).  The beam fades from opaque at the base to transparent at the
/// tip — as if light is emerging from the planet's surface.
fn draw_event_marker(
    painter: &egui::Painter,
    base: ProjectedPoint,
    tip: egui::Pos2,
    event: &EventRecord,
    is_selected: bool,
    time: f64,
) {
    let col = event.severity.color();
    let dx = tip.x - base.pos.x;
    let dy = tip.y - base.pos.y;

    // ── Wide atmospheric halos — full beam length, very low alpha ────────────
    // These give the diffuse glow without needing to be gradients.
    painter.line_segment([base.pos, tip], egui::Stroke::new(22.0, col.gamma_multiply(0.04)));
    painter.line_segment([base.pos, tip], egui::Stroke::new(11.0, col.gamma_multiply(0.08)));
    painter.line_segment([base.pos, tip], egui::Stroke::new(4.5,  col.gamma_multiply(0.16)));

    // ── Fading core — bright at the base, fading to nothing at the tip ───────
    // 6 segments with quadratic alpha falloff simulate a gradient.
    const SEGS: u32 = 6;
    for i in 0..SEGS {
        let t0 = i       as f32 / SEGS as f32;
        let t1 = (i + 1) as f32 / SEGS as f32;
        let alpha = (1.0 - t0).powi(2); // quadratic: 1.0 at base → 0.0 at tip
        let p0 = egui::pos2(base.pos.x + dx * t0, base.pos.y + dy * t0);
        let p1 = egui::pos2(base.pos.x + dx * t1, base.pos.y + dy * t1);
        painter.line_segment([p0, p1], egui::Stroke::new(3.2, col.gamma_multiply(alpha * 0.30)));
        painter.line_segment([p0, p1], egui::Stroke::new(1.4, col.gamma_multiply(alpha * 0.94)));
    }

    // ── Ground strike ────────────────────────────────────────────────────────
    if is_selected {
        let pulse = 9.0 + ((time as f32 * 2.6).sin() + 1.0) * 3.5;
        painter.circle_stroke(
            base.pos, pulse,
            egui::Stroke::new(1.3, theme::marker_glow_warm()),
        );
    }
    painter.circle_stroke(base.pos, 6.5, egui::Stroke::new(9.0,  col.gamma_multiply(0.06)));
    painter.circle_stroke(base.pos, 4.8, egui::Stroke::new(1.1,  col.gamma_multiply(0.60)));
    painter.circle_filled(base.pos, 2.5, col);
}

fn draw_camera_marker(painter: &egui::Painter, marker: ProjectedPoint, is_selected: bool) {
    let radius = 3.0 + marker.depth;
    let color = if is_selected { theme::marker_camera_ring() } else { theme::camera_color() };

    painter.circle_stroke(
        marker.pos, radius + 5.5,
        egui::Stroke::new(5.5, color.gamma_multiply(0.07)),
    );
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

/// Like `project_geo` but adds `extra_radius` (in globe-unit fractions) on
/// top of the terrain-based elevation.  Used to project a beam-tip point
/// directly above a geographic location so that the resulting screen-space
/// vector gives a perspective-correct beam direction: very short when the
/// event faces the camera, full-length when it is on the limb.
fn project_geo_elevated(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
    altitude_scale: f32,
    extra_radius: f32,
) -> Option<ProjectedPoint> {
    let lat = point.lat.to_radians();
    let lon = point.lon.to_radians();
    let elevation_signal = terrain_field::elevation(point) / 1.6;
    let signed_elevation = elevation_signal.mul_add(2.0, -1.0);
    let elevation = signed_elevation * altitude_scale;
    let radius = (1.0 + elevation + extra_radius).max(0.82);

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
