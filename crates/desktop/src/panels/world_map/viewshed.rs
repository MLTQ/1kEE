//! Real-time Viewshed / Line-of-Sight analysis with Earth curvature & refraction.
//!
//! Ships both a `@compute @workgroup_size(8, 8)` WGSL shader (`viewshed.wgsl`)
//! and an exact CPU grid evaluator + cached egui overlay renderer for Local view.

use crate::model::{GeoPoint, haversine_km};
use crate::panels::world_map::srtm_stream;
use crate::theme;
use egui::{Color32, Mesh, Pos2, Shape, Stroke, epaint::Vertex};
use std::cell::RefCell;
use std::path::Path;

/// Effective Earth radius accounting for standard atmospheric refraction (k ≈ 0.13).
pub const EARTH_R_EFF_M: f32 = 6_371_000.0 / 0.87;
pub const INV_2_R_EFF: f32 = 1.0 / (2.0 * EARTH_R_EFF_M);
pub const VIEWSHED_WGSL: &str = include_str!("viewshed.wgsl");

const FAN_RAYS: usize = 96;
const FAN_RINGS: usize = 28;

#[derive(Clone, Copy, Debug)]
struct FanSample {
    point: GeoPoint,
    elev_m: f32,
    visible: bool,
}

#[derive(Clone, Debug, Default)]
struct ViewshedCache {
    key: Option<(i32, i32, i32, i32)>,
    obs_ground_m: f32,
    samples: Vec<FanSample>, // length FAN_RAYS * FAN_RINGS (row-major: ray * FAN_RINGS + ring_idx)
    visible_fraction: f32,
}

#[derive(Clone, Debug)]
pub struct ViewshedState {
    pub enabled: bool,
    /// When true, the viewshed observer continuously tracks the Local Terrain camera center.
    pub follow_center: bool,
    pub observer: Option<GeoPoint>,
    /// Observer antenna / eye height above ground level (metres).
    pub observer_agl_m: f32,
    /// Analysis radius in kilometres.
    pub max_range_km: f32,
    cache: RefCell<ViewshedCache>,
}

impl Default for ViewshedState {
    fn default() -> Self {
        Self {
            enabled: false,
            follow_center: true,
            observer: None,
            observer_agl_m: 15.0,
            max_range_km: 35.0,
            cache: RefCell::new(ViewshedCache::default()),
        }
    }
}

impl ViewshedState {
    pub fn set_follow_center(&mut self) {
        self.follow_center = true;
        self.enabled = true;
        self.invalidate();
    }

    pub fn set_observer(&mut self, point: GeoPoint) {
        self.follow_center = false;
        self.observer = Some(point);
        self.enabled = true;
        self.invalidate();
    }

    pub fn clear(&mut self) {
        self.enabled = false;
        self.observer = None;
        let mut cache = self.cache.borrow_mut();
        cache.key = None;
        cache.samples.clear();
        cache.visible_fraction = 0.0;
    }

    pub fn invalidate(&mut self) {
        self.cache.borrow_mut().key = None;
    }

    pub fn visible_fraction(&self) -> f32 {
        self.cache.borrow().visible_fraction
    }
}

/// Evaluate line-of-sight on a regular `[w × h]` DEM height grid using Earth curvature
/// and standard atmospheric refraction (`h_eff = h - d² / (2 * R_eff)`).
pub fn compute_viewshed_grid(
    heights: &[f32],
    width: usize,
    height: usize,
    bounds: [f32; 4], // [west, south, east, north]
    observer: GeoPoint,
    observer_agl_m: f32,
    max_range_km: f32,
) -> Vec<u8> {
    let mut vis = vec![0u8; width * height];
    if width < 2 || height < 2 || heights.len() != width * height {
        return vis;
    }
    let [west, south, east, north] = bounds;
    let span_lon = (east - west).max(1e-6);
    let span_lat = (north - south).max(1e-6);
    let obs_u = ((observer.lon - west) / span_lon).clamp(0.0, 1.0);
    let obs_v = ((north - observer.lat) / span_lat).clamp(0.0, 1.0);
    let obs_ground = sample_grid(heights, width, height, obs_u, obs_v);
    let obs_elev = obs_ground + observer_agl_m;
    let max_range_m = max_range_km * 1000.0;
    let mid_lat = ((south + north) * 0.5).to_radians();
    let m_per_deg_lat = 111_195.0_f32;
    let m_per_deg_lon = m_per_deg_lat * mid_lat.cos().abs().max(0.05);

    for y in 0..height {
        let v = (y as f32 + 0.5) / height as f32;
        for x in 0..width {
            let u = (x as f32 + 0.5) / width as f32;
            let dx_m = (u - obs_u) * span_lon * m_per_deg_lon;
            let dy_m = -(v - obs_v) * span_lat * m_per_deg_lat;
            let dist_m = (dx_m * dx_m + dy_m * dy_m).sqrt();
            if dist_m > max_range_m {
                continue;
            }
            if dist_m < 25.0 {
                vis[y * width + x] = 2;
                continue;
            }
            let target_h = sample_grid(heights, width, height, u, v);
            let target_eff = target_h - dist_m * dist_m * INV_2_R_EFF;
            let target_angle = (target_eff - obs_elev) / dist_m;
            let steps = ((dist_m / 250.0).ceil() as usize).clamp(8, 64);
            let mut clear = true;
            for step in 1..steps {
                let t = step as f32 / steps as f32;
                let su = obs_u + (u - obs_u) * t;
                let sv = obs_v + (v - obs_v) * t;
                let sd = dist_m * t;
                let sh = sample_grid(heights, width, height, su, sv) - sd * sd * INV_2_R_EFF;
                let s_angle = (sh - obs_elev) / sd.max(1.0);
                if s_angle > target_angle {
                    clear = false;
                    break;
                }
            }
            vis[y * width + x] = if clear { 2 } else { 1 };
        }
    }
    vis
}

fn sample_grid(heights: &[f32], w: usize, h: usize, u: f32, v: f32) -> f32 {
    let fx = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let x0 = fx.floor() as usize;
    let y0 = fy.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let a = heights[y0 * w + x0];
    let b = heights[y0 * w + x1];
    let c = heights[y1 * w + x0];
    let d = heights[y1 * w + x1];
    (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty
}

/// Draw the cached terrain-draped radial viewshed fan plane in Local Terrain mode.
/// When `state.follow_center` is enabled, the observer tracks `viewport_center` live.
pub(crate) fn draw_local(
    painter: &egui::Painter,
    selected_root: Option<&Path>,
    state: &ViewshedState,
    viewport_center: GeoPoint,
    project: impl Fn(GeoPoint, f32) -> Option<Pos2>,
) {
    if !state.enabled {
        return;
    }
    let observer = if state.follow_center {
        viewport_center
    } else if let Some(obs) = state.observer {
        obs
    } else {
        return;
    };

    // Quantize to ~28 m (~1 SRTM arcsecond) so panning updates smoothly without jitter.
    let key = (
        (observer.lat * 4000.0).round() as i32,
        (observer.lon * 4000.0).round() as i32,
        state.observer_agl_m.round() as i32,
        (state.max_range_km * 10.0).round() as i32,
    );

    let mut cache = state.cache.borrow_mut();
    if cache.key != Some(key) {
        let obs_ground = srtm_stream::peek_elevation_m(selected_root, observer).unwrap_or_else(|| {
            srtm_stream::request_elevation_preload(selected_root, observer);
            0.0
        });
        let obs_elev = obs_ground + state.observer_agl_m;
        let deg_lat = state.max_range_km / 111.32;
        let deg_lon = deg_lat / observer.lat.to_radians().cos().abs().max(0.15);

        let mut samples = Vec::with_capacity(FAN_RAYS * FAN_RINGS);
        let mut vis_count = 0usize;
        for r in 0..FAN_RAYS {
            let az = (r as f32) * (std::f32::consts::TAU / FAN_RAYS as f32);
            let (sa, ca) = az.sin_cos();
            let mut max_horizon_angle = f32::NEG_INFINITY;
            for ring in 1..=FAN_RINGS {
                // Slightly denser rings near the observer for crisp foreground terrain draping.
                let frac = (ring as f32 / FAN_RINGS as f32).powf(1.25);
                let pt = GeoPoint {
                    lat: (observer.lat + ca * deg_lat * frac).clamp(-89.9, 89.9),
                    lon: observer.lon + sa * deg_lon * frac,
                };
                let dist_m = haversine_km(observer, pt) * 1000.0;
                let h = srtm_stream::peek_elevation_m(selected_root, pt).unwrap_or(obs_ground);
                let h_eff = h - dist_m * dist_m * INV_2_R_EFF;
                let angle = (h_eff - obs_elev) / dist_m.max(1.0);
                let visible = angle >= max_horizon_angle;
                if visible {
                    max_horizon_angle = angle;
                    vis_count += 1;
                }
                samples.push(FanSample {
                    point: pt,
                    elev_m: h,
                    visible,
                });
            }
        }
        cache.obs_ground_m = obs_ground;
        cache.visible_fraction = vis_count as f32 / (FAN_RAYS * FAN_RINGS) as f32;
        cache.samples = samples;
        cache.key = Some(key);
    }

    if cache.samples.len() != FAN_RAYS * FAN_RINGS {
        return;
    }

    const SURFACE_LIFT_M: f32 = 8.0;
    let obs_ground_pos = project(observer, cache.obs_ground_m + SURFACE_LIFT_M);
    let mut projected = vec![None::<Pos2>; FAN_RAYS * FAN_RINGS];
    for (idx, s) in cache.samples.iter().enumerate() {
        projected[idx] = project(s.point, s.elev_m + SURFACE_LIFT_M);
    }

    let is_light_theme = theme::atlas_day_weight() > 0.5;
    let (r_base, g_base, b_base) = if is_light_theme {
        (14u8, 150u8, 122u8)
    } else {
        (32u8, 240u8, 186u8)
    };
    let ring_color = |ring_idx: usize| -> Color32 {
        let t = ring_idx as f32 / FAN_RINGS as f32;
        let alpha = if is_light_theme {
            (92.0 * (1.0 - t * 0.72)).round() as u8
        } else {
            (80.0 * (1.0 - t * 0.74)).round() as u8
        };
        Color32::from_rgba_unmultiplied(r_base, g_base, b_base, alpha)
    };
    let center_color = Color32::from_rgba_unmultiplied(
        r_base,
        g_base,
        b_base,
        if is_light_theme { 112 } else { 104 },
    );

    // 1. Batched translucent terrain-draped visibility fan mesh.
    let mut mesh = Mesh::default();
    let push_tri = |mesh: &mut Mesh, a: (Pos2, Color32), b: (Pos2, Color32), c: (Pos2, Color32)| {
        let base = mesh.vertices.len() as u32;
        for (pos, color) in [a, b, c] {
            mesh.vertices.push(Vertex {
                pos,
                uv: Pos2::ZERO,
                color,
            });
        }
        mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
    };

    for ray in 0..FAN_RAYS {
        let next_ray = (ray + 1) % FAN_RAYS;
        // Inner wedge fanning directly from the observer center to ring 0.
        if let (Some(p_center), Some(p0), Some(p1)) = (
            obs_ground_pos,
            projected[ray * FAN_RINGS],
            projected[next_ray * FAN_RINGS],
        ) {
            let s0 = cache.samples[ray * FAN_RINGS];
            let s1 = cache.samples[next_ray * FAN_RINGS];
            if s0.visible || s1.visible {
                let c0 = ring_color(0);
                push_tri(&mut mesh, (p_center, center_color), (p0, c0), (p1, c0));
            }
        }
        // Concentric terrain-draped quads along the visible sectors.
        for ring in 0..(FAN_RINGS - 1) {
            let i00 = ray * FAN_RINGS + ring;
            let i10 = next_ray * FAN_RINGS + ring;
            let i01 = ray * FAN_RINGS + ring + 1;
            let i11 = next_ray * FAN_RINGS + ring + 1;
            let vis_corners = u8::from(cache.samples[i00].visible)
                + u8::from(cache.samples[i10].visible)
                + u8::from(cache.samples[i01].visible)
                + u8::from(cache.samples[i11].visible);
            if vis_corners < 3 {
                continue;
            }
            if let (Some(p00), Some(p10), Some(p11), Some(p01)) =
                (projected[i00], projected[i10], projected[i11], projected[i01])
            {
                let c_in = ring_color(ring);
                let c_out = ring_color(ring + 1);
                push_tri(&mut mesh, (p00, c_in), (p10, c_in), (p11, c_out));
                push_tri(&mut mesh, (p00, c_in), (p11, c_out), (p01, c_out));
            }
        }
    }
    if !mesh.indices.is_empty() {
        painter.add(Shape::mesh(mesh));
    }

    // 2. Subtle radial sightline spokes (12 azimuths, every 30°) drawn only where visible.
    let spoke_col = Color32::from_rgba_unmultiplied(
        r_base,
        g_base,
        b_base,
        if is_light_theme { 110 } else { 96 },
    );
    let spoke_stroke = Stroke::new(1.0, spoke_col);
    for ray in (0..FAN_RAYS).step_by(FAN_RAYS / 12) {
        let mut prev = obs_ground_pos;
        for ring in 0..FAN_RINGS {
            let idx = ray * FAN_RINGS + ring;
            let cur = projected[idx];
            if cache.samples[idx].visible {
                if let (Some(a), Some(b)) = (prev, cur) {
                    painter.line_segment([a, b], spoke_stroke);
                }
                prev = cur;
            } else {
                prev = None;
            }
        }
    }

    // 3. Crisp visibility horizon rim along ridge-crest cutoffs.
    let rim_col = Color32::from_rgba_unmultiplied(
        r_base,
        g_base,
        b_base,
        if is_light_theme { 190 } else { 180 },
    );
    let rim_stroke = Stroke::new(1.3, rim_col);
    for ray in 0..FAN_RAYS {
        let next_ray = (ray + 1) % FAN_RAYS;
        for ring in 0..(FAN_RINGS - 1) {
            let i0 = ray * FAN_RINGS + ring;
            let i1 = next_ray * FAN_RINGS + ring;
            let cutoff0 = cache.samples[i0].visible && !cache.samples[i0 + 1].visible;
            let cutoff1 = cache.samples[i1].visible && !cache.samples[i1 + 1].visible;
            if cutoff0 && cutoff1
                && let (Some(a), Some(b)) = (projected[i0], projected[i1])
            {
                painter.line_segment([a, b], rim_stroke);
            }
        }
    }

    // 4. Subtle outer range boundary ring (50% and 100% range).
    let range_col = Color32::from_rgba_unmultiplied(
        r_base,
        g_base,
        b_base,
        if is_light_theme { 72 } else { 58 },
    );
    let range_stroke = Stroke::new(0.85, range_col);
    for ring in [FAN_RINGS / 2 - 1, FAN_RINGS - 1] {
        for ray in 0..FAN_RAYS {
            let next_ray = (ray + 1) % FAN_RAYS;
            if let (Some(a), Some(b)) = (
                projected[ray * FAN_RINGS + ring],
                projected[next_ray * FAN_RINGS + ring],
            ) {
                painter.line_segment([a, b], range_stroke);
            }
        }
    }

    // 5. Observer elevation mast & center beacon.
    let beacon_col = if is_light_theme {
        Color32::from_rgb(10, 118, 98)
    } else {
        Color32::from_rgb(0, 245, 212)
    };
    let obs_tip_pos = project(observer, cache.obs_ground_m + state.observer_agl_m.max(SURFACE_LIFT_M));
    if let (Some(base), Some(tip)) = (obs_ground_pos, obs_tip_pos) {
        painter.line_segment([base, tip], Stroke::new(1.6, beacon_col.gamma_multiply(0.85)));
    }
    if let Some(pos) = obs_tip_pos.or(obs_ground_pos) {
        painter.circle_stroke(pos, 8.5, Stroke::new(1.7, beacon_col));
        painter.circle_filled(pos, 3.4, beacon_col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewshed_wgsl_parses_and_validates() {
        use eframe::wgpu::naga;
        let module = naga::front::wgsl::parse_str(VIEWSHED_WGSL).expect("viewshed WGSL parses");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("viewshed WGSL validates");
    }

    #[test]
    fn ridge_blocks_valley_behind_it_while_front_slope_remains_visible() {
        // 32x32 grid with a tall north-south ridge along column x=16
        let w = 32;
        let h = 32;
        let mut heights = vec![100.0_f32; w * h];
        for y in 0..h {
            heights[y * w + 16] = 1500.0;
        }
        let bounds = [0.0, 0.0, 0.32, 0.32];
        let observer = GeoPoint {
            lat: 0.16,
            lon: 0.08,
        };
        let vis = compute_viewshed_grid(&heights, w, h, bounds, observer, 20.0, 50.0);
        // Cell in front of ridge (x=12, y=16) must be visible (2)
        assert_eq!(vis[16 * w + 12], 2);
        // Ridge crest (x=16, y=16) must be visible (2)
        assert_eq!(vis[16 * w + 16], 2);
        // Valley directly behind ridge (x=24, y=16) must be shadowed (1)
        assert_eq!(vis[16 * w + 24], 1);
    }
}
