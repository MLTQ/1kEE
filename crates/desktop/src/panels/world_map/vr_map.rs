//! Render the geography alone, retaining the projection that produced its pixels.
use super::*;
use crate::vr_operator::{HEIGHT_EDGE, Hit, MapFrame, Target};
use std::sync::Arc;

pub(crate) fn paint(ui: &mut egui::Ui, model: &mut AppModel) -> MapFrame {
    let rect = ui.max_rect();
    let painter = ui.painter_at(rect);
    ui.ctx().data_mut(|d| {
        d.insert_temp(egui::Id::new("vr-local-capture"), true);
        d.remove::<String>(egui::Id::new("vr-terrain-status"));
    });
    poll_map_sources(ui, model);
    let local = local_terrain_scene::is_active(model);
    layer_import::ensure_visible_road_layers(model, local);
    layer_import::ensure_visible_water_layers(model, local);
    painter.add(atlas_surface::visibility_callback(rect));
    contour_pass::begin_frame();
    painter.add(contour_pass::residency_callback(rect));
    painter.add(local_contour_pass::residency_callback(rect, ui.ctx()));
    let saved = model.globe_view;
    let obs = model.obs_view;
    model.obs_view = true;
    let t = ui.ctx().input(|i| i.time);
    let mut frame = MapFrame::default();
    let scene = if local {
        // Render a north-up map. Stereo displacement is applied once by the headset.
        contour_asset::globe_residency::set_viewport(None, None);
        model.globe_view.local_pitch = 0.;
        model.globe_view.local_yaw = 0.;
        model.globe_view.local_layer_spread = 0.;
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("vr-local-capture"), true);
            d.remove::<Arc<atlas_surface::Raster>>(egui::Id::new("vr-local-raster"));
        });
        let scene = local_terrain_scene::paint(&painter, rect, model, t);
        let layout = local_terrain_scene::layout(rect);
        let center = saved.local_center;
        let half = local_terrain_scene::visual_half_extent_for_zoom(saved.local_zoom);
        let lon_km = 111.32 * center.lat.to_radians().cos().abs().max(0.2);
        let params = local_terrain_scene::projection::local_projection_params(
            &layout,
            &model.globe_view,
            center,
            (half * lon_km).max(1.),
            (half * 111.32).max(1.),
        );
        let dx = params.horizontal_scale * params.x_factor;
        let dy = params.ground_pitch_scale * params.y_factor;
        frame.bounds = [
            center.lon + (rect.left() - params.focus_center_x) / dx,
            center.lat - (rect.bottom() - params.focus_center_y) / dy,
            center.lon + (rect.right() - params.focus_center_x) / dx,
            center.lat - (rect.top() - params.focus_center_y) / dy,
        ];
        frame.local = true;
        let raster = ui
            .ctx()
            .data(|d| d.get_temp::<Arc<atlas_surface::Raster>>(egui::Id::new("vr-local-raster")));
        if let Some(raster) = raster {
            frame.heights = heights(&raster, frame.bounds, lon_km, saved.local_layer_spread);
        }
        scene
    } else {
        contour_asset::leave_local_view();
        let layout = globe_scene::globe_layout(rect, &saved);
        frame.projection = [
            (layout.center.x - rect.left()) / rect.width(),
            (layout.center.y - rect.top()) / rect.height(),
            layout.radius * layout.focal_length / rect.width(),
            layout.camera_distance,
        ];
        frame.angles = [saved.yaw, saved.pitch];
        globe_scene::paint(&painter, rect, model, t)
    };
    model.globe_view = saved;
    model.obs_view = obs;
    contour_pass::end_frame();
    for (markers, camera) in [(scene.event_markers, false), (scene.camera_markers, true)] {
        for m in markers {
            if !rect.contains(m.base) {
                continue;
            }
            let uv = |p: egui::Pos2| {
                [
                    (p.x - rect.left()) / rect.width(),
                    (p.y - rect.top()) / rect.height(),
                ]
            };
            frame.hits.push(Hit {
                base: uv(m.base),
                tip: uv(m.tip),
                radius: (10. * m.scale).max(5.) / rect.width(),
                target: if camera {
                    Target::Camera(m.id)
                } else {
                    Target::Event(m.id)
                },
            });
        }
    }
    frame
}

fn heights(r: &atlas_surface::Raster, b: [f32; 4], lon_km: f32, spread: f32) -> Vec<f32> {
    let [w, h] = r.size;
    let [west, south, east, north] = r.bounds;
    let width_m = (b[2] - b[0]) * lon_km * 1000.;
    let mut out = Vec::with_capacity(HEIGHT_EDGE * HEIGHT_EDGE);
    for row in 0..HEIGHT_EDGE {
        let lat = b[3] - (b[3] - b[1]) * row as f32 / (HEIGHT_EDGE - 1) as f32;
        for col in 0..HEIGHT_EDGE {
            let lon = b[0] + (b[2] - b[0]) * col as f32 / (HEIGHT_EDGE - 1) as f32;
            let lon = lon + (((west + east) * 0.5 - lon) / 360.).round() * 360.;
            let u = (lon - west) / (east - west);
            let v = (north - lat) / (north - south);
            let mut z = f32::NAN;
            if (0.0..=1.).contains(&u) && (0.0..=1.).contains(&v) {
                let m = r.heights[(v * (h - 1) as f32).round() as usize * w
                    + (u * (w - 1) as f32).round() as usize];
                if (-11000.0..=9000.0).contains(&m) {
                    z = m;
                }
            }
            out.push(z);
        }
    }
    let (lo, hi) = out
        .iter()
        .copied()
        .filter(|z| z.is_finite())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), z| {
            (lo.min(z), hi.max(z))
        });
    if !lo.is_finite() {
        return Vec::new();
    }
    let gain = (spread * 2.1 / width_m).min(0.46 / (hi - lo).max(1.));
    for value in &mut out {
        *value = if value.is_finite() {
            (*value - (lo + hi) * 0.5) * gain
        } else {
            0.
        };
    }
    out
}
