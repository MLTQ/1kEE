//! Render the geography alone, retaining the projection that produced its pixels.
use super::*;
use crate::model::{GeoPoint, ProjectionMode};
use crate::vr_operator::{GlobeCapture, HEIGHT_EDGE, LOCAL_TABLE_RANGE_M, MapFrame, Spire, Target};
use std::sync::Arc;

pub(crate) fn paint(
    ui: &mut egui::Ui,
    model: &mut AppModel,
    capture: Option<GlobeCapture>,
    globe_scale: f32,
) -> MapFrame {
    let rect = ui.max_rect();
    let painter = ui.painter_at(rect);
    ui.ctx().data_mut(|d| {
        d.insert_temp(egui::Id::new("vr-local-capture"), true);
        d.remove::<String>(egui::Id::new("vr-terrain-status"));
    });
    poll_map_sources(ui, model);
    let local = model.globe_view.range_m <= LOCAL_TABLE_RANGE_M || model.globe_view.local_mode;
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
    let mut frame = MapFrame {
        marker_scale: model.marker_scale(),
        globe_scale,
        ..MapFrame::default()
    };
    if local {
        // Render a north-up orthographic capture using the unified scene so
        // Atlas terrain, contours, OSM, and the fused LiDAR point cloud (1 px)
        // all render seamlessly down to 20 m scale. True 3D stereo height
        // displacement is applied once by the headset's raymarch shader.
        contour_asset::globe_residency::set_viewport(None, None);
        model.globe_view.geographic_capture = true;
        model.globe_view.projection = ProjectionMode::Orthographic;
        model.globe_view.heading = 0.0;
        model.globe_view.tilt = 0.0;
        model.globe_view.local_pitch = 0.0;
        model.globe_view.local_yaw = 0.0;
        model.globe_view.local_layer_spread = 0.0;
        model.globe_view.range_m = saved.range_m.clamp(20.0, LOCAL_TABLE_RANGE_M);
        model.globe_view.sync_legacy_from_unified();
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("vr-local-capture"), true);
            d.remove::<Arc<atlas_surface::Raster>>(egui::Id::new("vr-local-raster"));
        });
        let proj = projection::MapProjection::from_view(
            &model.globe_view,
            model.active_body,
            rect,
            ui.ctx().pixels_per_point(),
        );
        unified_scene::paint(&painter, rect, model, t);
        let center_lat = model.globe_view.target_lat as f32;
        let center_lon = model.globe_view.target_lon as f32;
        let lon_km = 111.32 * center_lat.to_radians().cos().abs().max(0.2);
        if let Some(vis) = proj.visible_bounds() {
            frame.bounds = [vis[0] as f32, vis[1] as f32, vis[2] as f32, vis[3] as f32];
        } else {
            let half_lat = (proj.visible_half_extent_deg() as f32).max(1e-4);
            let half_lon = half_lat / center_lat.to_radians().cos().abs().max(0.2);
            frame.bounds = [
                center_lon - half_lon,
                (center_lat - half_lat).max(-89.9),
                center_lon + half_lon,
                (center_lat + half_lat).min(89.9),
            ];
        }
        frame.local = true;
        let raster = ui
            .ctx()
            .data(|d| d.get_temp::<Arc<atlas_surface::Raster>>(egui::Id::new("vr-local-raster")));
        if let Some(raster) = raster {
            frame.heights = heights(&raster, frame.bounds, lon_km, saved.local_layer_spread);
        }
    } else {
        contour_asset::leave_local_view();
        let mut layout = globe_scene::globe_layout(rect, &saved);
        if let Some(c) = capture {
            model.globe_view.yaw = c.lon.to_radians() - std::f32::consts::FRAC_PI_2;
            model.globe_view.pitch = c.lat.to_radians();
            model.globe_view.local_center = GeoPoint {
                lat: c.lat,
                lon: c.lon,
            };
            // Fit the entire silhouette to the square capture. The guard angle
            // beyond both eyes' horizons accommodates IPD and small head movements.
            layout.camera_distance = c.distance;
            layout.center = rect.center();
            layout.radius =
                rect.width() * 0.48 * (c.distance * c.distance - 1.).sqrt() / layout.focal_length;
        }
        frame.projection = [
            (layout.center.x - rect.left()) / rect.width(),
            (layout.center.y - rect.top()) / rect.height(),
            layout.radius * layout.focal_length / rect.width(),
            layout.camera_distance,
        ];
        frame.angles = [model.globe_view.yaw, model.globe_view.pitch];
        globe_scene::paint_with_layout(&painter, rect, model, t, layout, true);
    };
    model.globe_view = saved;
    model.obs_view = obs;
    contour_pass::end_frame();
    if model.active_body == crate::model::ActiveBody::Earth {
        let mut push = |p: GeoPoint, color: egui::Color32, target: Option<Target>| {
            let [west, south, east, north] = frame.bounds;
            let lon = p.lon + (((west + east) * 0.5 - p.lon) / 360.).round() * 360.;
            if p.lat.is_finite()
                && lon.is_finite()
                && (!local || ((west..=east).contains(&lon) && (south..=north).contains(&p.lat)))
            {
                frame.spires.push(Spire {
                    location: [p.lat, lon],
                    color: [color.r(), color.g(), color.b()],
                    target,
                });
            }
        };
        if !local && model.replay_mode {
            if let Some(replay) = model.active_replay() {
                for flare in &replay.active_flares {
                    let alpha = flare.alpha(replay.wall_elapsed());
                    if alpha > 0.01 {
                        push(
                            flare.event.location,
                            flare.event.severity.color().gamma_multiply(alpha),
                            None,
                        );
                    }
                }
            }
        } else if model.show_event_markers && (local || model.follow_recaps.active.is_none()) {
            for e in model.map_events() {
                let selected = model.selected_event_id.as_deref() == Some(&e.id);
                push(
                    e.location,
                    if selected {
                        egui::Color32::WHITE
                    } else {
                        e.severity.color()
                    },
                    Some(Target::Event(e.id.clone())),
                );
            }
        }
        if model.show_camera_markers {
            for c in &model.cameras {
                let selected = model.selected_camera_id.as_deref() == Some(&c.id);
                push(
                    c.location,
                    if selected {
                        egui::Color32::WHITE
                    } else {
                        crate::theme::camera_color()
                    },
                    Some(Target::Camera(c.id.clone())),
                );
            }
        }
    }
    frame
}

fn heights(r: &atlas_surface::Raster, b: [f32; 4], lon_km: f32, spread: f32) -> Vec<f32> {
    let [w, h] = r.size;
    let [west, south, east, north] = r.bounds;
    let width_m = ((b[2] - b[0]) * lon_km * 1000.).max(1.0);
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
