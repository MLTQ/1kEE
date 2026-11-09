//! Stable, padded geographic windows for measured Atlas detail.
use super::{
    globe_scene::{GlobeLayout, project_surface, screen_to_latlon},
    local_terrain_scene::{LocalLayout, projection::local_projection_params},
};
use crate::model::{GeoPoint, GlobeViewState};

/// Arcsecond edges give exact equality without tying detail to each mouse pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Region(pub [i32; 4]);
impl Region {
    pub fn padded(bounds: [f32; 4]) -> Option<Self> {
        if !bounds.iter().all(|v| v.is_finite()) || bounds[0] >= bounds[2] || bounds[1] >= bounds[3]
        {
            return None;
        }
        let span = (bounds[2] - bounds[0]).max(bounds[3] - bounds[1]);
        let step = (2f32.powf((span * 3600.0 / 16.0).log2().floor())).max(1.0);
        let margin = span * 0.12 * 3600.0 + step;
        let floor = |v: f32| ((v * 3600.0 - margin) / step).floor() as i32 * step as i32;
        let ceil = |v: f32| ((v * 3600.0 + margin) / step).ceil() as i32 * step as i32;
        let west = floor(bounds[0]);
        Some(Self([
            west,
            floor(bounds[1]).max(-324000),
            ceil(bounds[2]).min(west + 1296000),
            ceil(bounds[3]).min(324000),
        ]))
    }
    pub fn contains(self, mut bounds: [f32; 4]) -> bool {
        let [w, south, e, north] = self.bounds();
        let shift = (((w + e) - (bounds[0] + bounds[2])) / 720.0).round() * 360.0;
        bounds[0] += shift;
        bounds[2] += shift;
        bounds.iter().all(|v| v.is_finite())
            && bounds[0] >= w
            && bounds[2] <= e
            && bounds[1] >= south
            && bounds[3] <= north
    }
    pub fn bounds(self) -> [f32; 4] {
        self.0.map(|v| v as f32 / 3600.0)
    }
    pub fn span(self) -> [f32; 2] {
        [
            (self.0[2] - self.0[0]) as f32 / 3600.0,
            (self.0[3] - self.0[1]) as f32 / 3600.0,
        ]
    }
    /// Read native posts when they fit. Bound peak image memory and GPU edge size.
    pub fn size(self, native_arcseconds: f32, max_edge: usize) -> [usize; 2] {
        let raw = [
            (self.0[2] - self.0[0]) as f64 / native_arcseconds as f64,
            (self.0[3] - self.0[1]) as f64 / native_arcseconds as f64,
        ];
        let edge = max_edge.clamp(16, 8192) as f64;
        let factor = (edge / raw[0])
            .min(edge / raw[1])
            .min((32_000_000.0 / (raw[0] * raw[1])).sqrt())
            .min(1.0);
        let mut size = raw.map(|v| (v * factor).ceil().max(16.0) as usize);
        // Ceil on both axes can exceed the reader's strict pixel budget.
        if size[0] * size[1] > 32_000_000 {
            size[1] = 32_000_000 / size[0];
        }
        size
    }
}

pub(super) fn globe_visible(
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    ppp: f32,
) -> Option<[f32; 4]> {
    let scale = layout.radius * layout.focal_length / (layout.camera_distance - 1.0);
    if scale * std::f32::consts::TAU / 4096.0 * ppp < 0.7 {
        return None;
    }
    globe_bounds(rect, layout, view)
}

/// Visible coverage even when the overview is sufficient: retained tiles still draw.
pub(super) fn globe_bounds(
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
) -> Option<[f32; 4]> {
    let center = view.globe_center_latlon();
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    let mut include = |pos| {
        if let Some(p) = screen_to_latlon(rect, view, pos) {
            let lon = center.lon + (p.lon - center.lon + 180.0).rem_euclid(360.0) - 180.0;
            bounds[0] = bounds[0].min(lon);
            bounds[1] = bounds[1].min(p.lat);
            bounds[2] = bounds[2].max(lon);
            bounds[3] = bounds[3].max(p.lat);
        }
    };
    for row in 0..=16 {
        for col in 0..=16 {
            include(egui::pos2(
                rect.left() + rect.width() * col as f32 / 16.0,
                rect.top() + rect.height() * row as f32 / 16.0,
            ));
        }
    }
    let radius = layout.radius * layout.focal_length
        / (layout.camera_distance.powi(2) - 1.0).sqrt()
        * 0.9999;
    for i in 0..128 {
        let a = i as f32 * std::f32::consts::TAU / 128.0;
        let p = layout.center + egui::vec2(a.cos(), a.sin()) * radius;
        if rect.contains(p) {
            include(p);
        }
    }
    for lat in [-90.0, 90.0] {
        if project_surface(
            layout,
            view,
            GeoPoint {
                lat,
                lon: center.lon,
            },
        )
        .is_some_and(|p| {
            p.depth * 2.0 - 1.0 >= 1.0 / layout.camera_distance && rect.contains(p.pos)
        }) {
            bounds[0] = -180.0;
            bounds[2] = 180.0;
            if lat > 0.0 {
                bounds[3] = 90.0;
            } else {
                bounds[1] = -90.0;
            }
        }
    }
    bounds.iter().all(|v| v.is_finite()).then_some(bounds)
}

pub(super) fn local_visible(
    clip: egui::Rect,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    half: f32,
) -> Option<[f32; 4]> {
    let ex = (half * 111.32 * focus.lat.to_radians().cos().abs().max(0.2)).max(1.0);
    let projection = local_projection_params(layout, view, focus, ex, (half * 111.32).max(1.0));
    let project = |lat, lon| {
        let (x, y, _) = projection.project_logical(GeoPoint { lat, lon }, 0.0);
        egui::pos2(x, y)
    };
    let origin = project(focus.lat, focus.lon);
    let u = project(focus.lat, focus.lon + 1.0) - origin;
    let v = project(focus.lat + 1.0, focus.lon) - origin;
    let det = u.x * v.y - u.y * v.x;
    if !det.is_finite() || det.abs() < 1e-6 {
        return None;
    }
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for p in [
        clip.left_top(),
        clip.right_top(),
        clip.left_bottom(),
        clip.right_bottom(),
    ] {
        let d = p - origin;
        let lon = focus.lon + (d.x * v.y - d.y * v.x) / det;
        let lat = focus.lat + (u.x * d.y - u.y * d.x) / det;
        bounds[0] = bounds[0].min(lon);
        bounds[1] = bounds[1].min(lat);
        bounds[2] = bounds[2].max(lon);
        bounds[3] = bounds[3].max(lat);
    }
    Some(bounds)
}

#[cfg(test)]
#[path = "atlas_viewport_tests.rs"]
mod tests;

#[cfg(test)]
pub(super) fn globe(
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    ppp: f32,
) -> Option<Region> {
    Region::padded(globe_visible(rect, layout, view, ppp)?)
}
#[cfg(test)]
pub(super) fn local(
    clip: egui::Rect,
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    half: f32,
) -> Option<Region> {
    Region::padded(local_visible(clip, layout, view, focus, half)?)
}
