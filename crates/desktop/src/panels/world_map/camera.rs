use crate::model::GlobeViewState;

use super::local_terrain_scene;

const GLOBE_PITCH_LIMIT_RAD: f32 = 1.53;

/// Half-life for momentum decay in seconds.
/// After this many seconds the velocity has dropped to 50% of its release value.
const MOMENTUM_HALF_LIFE: f32 = 0.28;

/// Minimum velocity magnitude before it is zeroed out (prevents endless micro-repaints).
const DEAD_VEL: f32 = 0.0005;
const DEAD_PAN: f32 = 0.000_02; // degrees/s

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
    // Per-frame timestep — clamped to prevent physics explosions after pauses/debugging.
    let dt = ctx.input(|i| i.stable_dt).clamp(1.0 / 240.0, 1.0 / 20.0);

    // Whether any live input is driving the velocity this frame.
    // When true, decay is skipped so momentum doesn't fight active input.
    let mut input_active = false;

    // ── Mouse drag ────────────────────────────────────────────────────────────
    if response.dragged() {
        input_active = true;
        let raw_delta = ctx.input(|i| i.pointer.delta());
        let delta = egui::Vec2::new(
            raw_delta.x.clamp(-32.0, 32.0),
            raw_delta.y.clamp(-32.0, 32.0),
        );
        let rotate_mod = ctx.input(|i| i.modifiers.ctrl || i.modifiers.shift);

        if view.local_mode {
            if rotate_mod {
                // Ctrl/Shift drag → rotate camera angle.
                // Convert pixel delta to rad/s and blend into velocity.
                let iv_yaw = -delta.x * 0.0085 / dt;
                let iv_pitch = -delta.y * 0.006 / dt;
                view.vel_local_yaw = lerp(view.vel_local_yaw, iv_yaw, 0.88);
                view.vel_local_pitch = lerp(view.vel_local_pitch, iv_pitch, 0.88);
                // Dampen pan momentum when mode switches to rotate.
                view.vel_local_lat *= 0.3;
                view.vel_local_lon *= 0.3;
            } else {
                // Plain drag → pan.  Convert pixel delta to lat/lon deg and
                // divide by dt to get velocity in deg/s.
                let (dlat, dlon) = local_pan_delta_to_latlon(response.rect, view, delta);
                view.vel_local_lat = lerp(view.vel_local_lat, dlat / dt, 0.88);
                view.vel_local_lon = lerp(view.vel_local_lon, dlon / dt, 0.88);
                // Dampen rotate momentum when switching to pan.
                view.vel_local_yaw *= 0.3;
                view.vel_local_pitch *= 0.3;
            }
        } else {
            let iv_yaw = -delta.x * 0.0055 / dt;
            let iv_pitch = delta.y * 0.004 / dt;
            view.vel_yaw = lerp(view.vel_yaw, iv_yaw, 0.88);
            view.vel_pitch = lerp(view.vel_pitch, iv_pitch, 0.88);
        }
        view.auto_spin = false;
    }

    // ── Scroll zoom (no momentum — instant feels best for zoom) ──────────────
    let scroll_y = ctx.input(|i| {
        if response.hovered() { i.raw_scroll_delta.y } else { 0.0 }
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

    // ── Keyboard arrow navigation ─────────────────────────────────────────────
    if response.hovered() {
        let (left, right, up, down, rotate_mod) = ctx.input(|i| (
            i.key_down(egui::Key::ArrowLeft),
            i.key_down(egui::Key::ArrowRight),
            i.key_down(egui::Key::ArrowUp),
            i.key_down(egui::Key::ArrowDown),
            i.modifiers.ctrl || i.modifiers.shift,
        ));

        if left || right || up || down {
            // Accumulate hold time and compute an acceleration ramp.
            // sqrt gives a fast initial rise that flattens at full speed,
            // so a short tap is a small nudge while holding builds up pace.
            // Full speed is reached after KEY_RAMP_SECS seconds.
            const KEY_RAMP_SECS: f32 = 1.8;
            view.key_hold_secs = (view.key_hold_secs + dt).min(KEY_RAMP_SECS);
            let ramp = (view.key_hold_secs / KEY_RAMP_SECS).sqrt();

            input_active = true;
            let h = if left { -1.0f32 } else if right { 1.0 } else { 0.0 };
            let v = if up { -1.0f32 } else if down { 1.0 } else { 0.0 };

            if view.local_mode {
                if rotate_mod {
                    // Ctrl/Shift + arrows → rotate camera
                    view.vel_local_yaw = lerp(view.vel_local_yaw, h * 1.1 * ramp, 0.5);
                    view.vel_local_pitch = lerp(view.vel_local_pitch, v * 0.8 * ramp, 0.5);
                    view.vel_local_lat *= 0.9;
                    view.vel_local_lon *= 0.9;
                } else {
                    // Plain arrows → pan.  Scale the px/s target by ramp.
                    let key_px = egui::Vec2::new(-h * 160.0 * ramp, -v * 160.0 * ramp);
                    let (lat_ps, lon_ps) = local_pan_delta_to_latlon(response.rect, view, key_px);
                    view.vel_local_lat = lerp(view.vel_local_lat, lat_ps, 0.5);
                    view.vel_local_lon = lerp(view.vel_local_lon, lon_ps, 0.5);
                    view.vel_local_yaw *= 0.9;
                    view.vel_local_pitch *= 0.9;
                }
            } else {
                let rate = 1.1 / view.zoom.sqrt().max(0.5) * ramp;
                view.vel_yaw = lerp(view.vel_yaw, h * rate, 0.5);
                view.vel_pitch = lerp(view.vel_pitch, -v * rate * 0.72, 0.5);
            }
            view.auto_spin = false;
        } else {
            // All keys released — reset ramp so the next tap starts slow again.
            view.key_hold_secs = 0.0;
        }
    } else {
        view.key_hold_secs = 0.0;
    }

    // ── Auto-spin ────────────────────────────────────────────────────────────
    if view.auto_spin && !response.hovered() {
        // Override vel_yaw with the constant spin speed; skip decay.
        view.vel_yaw = -0.18;
        input_active = true;
    }

    // ── Momentum decay ───────────────────────────────────────────────────────
    // Only decay when no input is actively driving the velocity.
    if !input_active {
        let decay = 0.5f32.powf(dt / MOMENTUM_HALF_LIFE);
        view.vel_yaw *= decay;
        view.vel_pitch *= decay;
        view.vel_local_lat *= decay;
        view.vel_local_lon *= decay;
        view.vel_local_yaw *= decay;
        view.vel_local_pitch *= decay;
    }

    // Dead-zone: kill negligible velocities so we stop requesting repaints.
    if view.vel_yaw.abs() < DEAD_VEL { view.vel_yaw = 0.0; }
    if view.vel_pitch.abs() < DEAD_VEL { view.vel_pitch = 0.0; }
    if view.vel_local_yaw.abs() < DEAD_VEL { view.vel_local_yaw = 0.0; }
    if view.vel_local_pitch.abs() < DEAD_VEL { view.vel_local_pitch = 0.0; }
    if view.vel_local_lat.abs() < DEAD_PAN { view.vel_local_lat = 0.0; }
    if view.vel_local_lon.abs() < DEAD_PAN { view.vel_local_lon = 0.0; }

    // ── Apply velocity to position ───────────────────────────────────────────
    if view.local_mode {
        view.local_yaw += view.vel_local_yaw * dt;
        view.local_pitch =
            (view.local_pitch + view.vel_local_pitch * dt).clamp(0.02, 1.55);
        view.local_center.lat =
            (view.local_center.lat + view.vel_local_lat * dt).clamp(-85.0, 85.0);
        view.local_center.lon =
            normalize_lon(view.local_center.lon + view.vel_local_lon * dt);
    } else {
        view.yaw += view.vel_yaw * dt;
        view.pitch = (view.pitch + view.vel_pitch * dt)
            .clamp(-GLOBE_PITCH_LIMIT_RAD, GLOBE_PITCH_LIMIT_RAD);
    }

    // Request repaint while coasting so the view updates every frame.
    let coasting = view.vel_yaw != 0.0
        || view.vel_pitch != 0.0
        || view.vel_local_lat != 0.0
        || view.vel_local_lon != 0.0
        || view.vel_local_yaw != 0.0
        || view.vel_local_pitch != 0.0;
    if coasting {
        ctx.request_repaint();
    }

    // Keep local_center in sync with the globe viewport while in globe mode,
    // so switching to local renders the area the user is looking at.
    if !view.local_mode {
        view.local_center = view.globe_center_latlon();
    }
}

/// Convert a pixel-space delta (or velocity in px/s) into a lat/lon displacement
/// (or velocity in deg/s) using the same coordinate mapping as the local-mode renderer.
/// Passing `delta_px` (pixels) returns degrees of displacement.
/// Passing `vel_px` (pixels/second) returns degrees/second velocity.
fn local_pan_delta_to_latlon(
    rect: egui::Rect,
    view: &GlobeViewState,
    delta_px: egui::Vec2,
) -> (f32, f32) {
    let half_extent_deg = local_terrain_scene::visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon =
        km_per_deg_lat * view.local_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);
    let horizontal_scale = rect.width() * 0.31;
    let ground_vertical_scale =
        rect.height() * 0.74 * 0.55 * view.local_pitch.cos() - 48.0 * view.local_pitch.sin();
    let vertical_scale = ground_vertical_scale.abs().max(18.0);

    let x_yaw_shift = -delta_px.x / horizontal_scale.max(1.0);
    let y_yaw_shift = delta_px.y / vertical_scale;
    let yaw_cos = view.local_yaw.cos();
    let yaw_sin = view.local_yaw.sin();
    let x_shift = x_yaw_shift * yaw_cos + y_yaw_shift * yaw_sin;
    let y_shift = -x_yaw_shift * yaw_sin + y_yaw_shift * yaw_cos;

    let east_km = x_shift * extent_x_km;
    let north_km = y_shift * extent_y_km;
    let dlat = north_km / km_per_deg_lat;
    let dlon = east_km / km_per_deg_lon.max(8.0);
    (dlat, dlon)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn normalize_lon(lon: f32) -> f32 {
    let mut wrapped = lon;
    while wrapped > 180.0 { wrapped -= 360.0; }
    while wrapped < -180.0 { wrapped += 360.0; }
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
