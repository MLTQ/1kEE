use crate::model::GlobeViewState;

use super::local_terrain_scene;

const GLOBE_PITCH_LIMIT_RAD: f32 = 1.53;

pub struct GlobeLod {
    pub lat_line_step: usize,
    pub lon_line_step: usize,
    pub sample_step: usize,
    pub contour_layers: usize,
    pub altitude_scale: f32,
    pub backface_alpha: f32,
}

pub fn apply_interaction(
    ctx: &egui::Context,
    response: &egui::Response,
    view: &mut GlobeViewState,
) {
    if response.dragged() {
        let raw_delta = ctx.input(|input| input.pointer.delta());
        // Clamp per-frame delta to prevent huge jumps during lag recovery.
        // At 60fps a full-screen drag is ~16px/frame; cap at 32 for headroom.
        let delta = egui::Vec2::new(
            raw_delta.x.clamp(-32.0, 32.0),
            raw_delta.y.clamp(-32.0, 32.0),
        );
        if view.local_mode {
            let rotate_mode = ctx.input(|input| input.modifiers.ctrl || input.modifiers.shift);
            if rotate_mode {
                view.local_yaw -= delta.x * 0.0085;
                view.local_pitch = (view.local_pitch - delta.y * 0.006).clamp(0.35, 1.35);
            } else {
                pan_local_center(response.rect, view, delta);
            }
        } else {
            view.yaw -= delta.x * 0.0055;
            view.pitch =
                (view.pitch + delta.y * 0.004).clamp(-GLOBE_PITCH_LIMIT_RAD, GLOBE_PITCH_LIMIT_RAD);
        }
        view.auto_spin = false;
    }

    let scroll_y = ctx.input(|input| {
        if response.hovered() {
            input.raw_scroll_delta.y
        } else {
            0.0
        }
    });

    if scroll_y.abs() > f32::EPSILON {
        if view.local_mode {
            view.local_zoom = (view.local_zoom * (scroll_y * 0.0055).exp())
                .clamp(local_terrain_scene::LOCAL_ZOOM_MIN, 60.0);
        } else {
            view.zoom = (view.zoom * (scroll_y * 0.0055).exp()).clamp(0.6, 50.0);
        }
        view.auto_spin = false;
    }

    if view.auto_spin && !response.hovered() {
        let dt = ctx.input(|input| input.stable_dt).max(1.0 / 120.0);
        view.yaw -= dt * 0.18;
    }

    // ── Keyboard arrow navigation (active while map is hovered) ──────────────
    if response.hovered() {
        let dt = ctx.input(|input| input.stable_dt).clamp(0.0, 0.05);
        let (left, right, up, down, rotate_mod) = ctx.input(|input| (
            input.key_down(egui::Key::ArrowLeft),
            input.key_down(egui::Key::ArrowRight),
            input.key_down(egui::Key::ArrowUp),
            input.key_down(egui::Key::ArrowDown),
            input.modifiers.ctrl || input.modifiers.shift,
        ));

        if left || right || up || down {
            let h = if left { -1.0f32 } else if right { 1.0 } else { 0.0 };
            let v = if up { -1.0f32 } else if down { 1.0 } else { 0.0 };

            if view.local_mode {
                if rotate_mod {
                    // Ctrl/Shift + arrows → rotate camera angle
                    view.local_yaw -= h * dt * 1.6;
                    view.local_pitch =
                        (view.local_pitch - v * dt * 1.1).clamp(0.35, 1.35);
                } else {
                    // Plain arrows → pan using same logic as mouse drag
                    // 180 px/s key speed gives a comfortable pan rate at any zoom level.
                    let key_px = 180.0 * dt;
                    pan_local_center(
                        response.rect,
                        view,
                        egui::Vec2::new(h * key_px, v * key_px),
                    );
                }
            } else {
                // Globe mode: rotate yaw/pitch, speed eases down as zoom grows.
                let rate = 1.4 / view.zoom.sqrt().max(0.5);
                view.yaw -= h * dt * rate;
                view.pitch = (view.pitch + v * dt * rate * 0.72)
                    .clamp(-GLOBE_PITCH_LIMIT_RAD, GLOBE_PITCH_LIMIT_RAD);
            }
            view.auto_spin = false;
            ctx.request_repaint();
        }
    }

    // Keep local_center in sync with the globe viewport while in globe mode,
    // so switching to local renders the area the user is looking at.
    if !view.local_mode {
        view.local_center = view.globe_center_latlon();
    }
}

fn pan_local_center(rect: egui::Rect, view: &mut GlobeViewState, delta: egui::Vec2) {
    let render_zoom = local_terrain_scene::local_render_zoom(view.local_zoom);
    let half_extent_deg = local_terrain_scene::visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * view.local_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);
    let horizontal_scale = rect.width() * 0.31;
    let ground_vertical_scale =
        rect.height() * 0.74 * 0.55 * view.local_pitch.cos() - 48.0 * view.local_pitch.sin();
    let vertical_scale = ground_vertical_scale.abs().max(18.0);

    let x_yaw_shift = -delta.x / horizontal_scale.max(1.0);
    let y_yaw_shift = delta.y / vertical_scale; // positive: drag down → center moves north (toward top)

    let yaw_cos = view.local_yaw.cos();
    let yaw_sin = view.local_yaw.sin();
    let x_shift = x_yaw_shift * yaw_cos + y_yaw_shift * yaw_sin;
    let y_shift = -x_yaw_shift * yaw_sin + y_yaw_shift * yaw_cos;

    let east_km = x_shift * extent_x_km;
    let north_km = y_shift * extent_y_km;

    view.local_center.lat = (view.local_center.lat + north_km / km_per_deg_lat).clamp(-85.0, 85.0);
    let lon_scale = km_per_deg_lon.max(8.0);
    view.local_center.lon = normalize_lon(view.local_center.lon + east_km / lon_scale);
}

fn normalize_lon(lon: f32) -> f32 {
    let mut wrapped = lon;
    while wrapped > 180.0 {
        wrapped -= 360.0;
    }
    while wrapped < -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

pub fn lod(view: &GlobeViewState) -> GlobeLod {
    if view.zoom < 1.0 {
        GlobeLod {
            lat_line_step: 24,
            lon_line_step: 24,
            sample_step: 10,
            contour_layers: 10,
            altitude_scale: 0.045,
            backface_alpha: 0.18,
        }
    } else if view.zoom < 2.5 {
        GlobeLod {
            lat_line_step: 18,
            lon_line_step: 18,
            sample_step: 8,
            contour_layers: 14,
            altitude_scale: 0.065,
            backface_alpha: 0.14,
        }
    } else if view.zoom < 5.0 {
        GlobeLod {
            lat_line_step: 24,
            lon_line_step: 24,
            sample_step: 6,
            contour_layers: 20,
            altitude_scale: 0.11,
            backface_alpha: 0.08,
        }
    } else {
        GlobeLod {
            lat_line_step: 45,
            lon_line_step: 45,
            sample_step: 8,
            contour_layers: 24,
            altitude_scale: 0.14,
            backface_alpha: 0.06,
        }
    }
}
