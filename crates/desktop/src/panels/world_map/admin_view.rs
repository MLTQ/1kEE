//! Quantized detail and conservative geographic coverage for boundary workers.
use super::super::{globe_scene::GlobeLayout, local_terrain_scene::projection::LocalProjectionParams};
use crate::model::GlobeViewState;
use tile_archive::admin_lod::{Window, tier_for_scale};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::panels::world_map) struct Request {
    pub tier: usize,
    pub window: Window,
    pub shade: bool,
}
impl Request {
    pub fn globe(
        layout: &GlobeLayout,
        view: &GlobeViewState,
        rect: egui::Rect,
        ppp: f32,
        shade: bool,
    ) -> Self {
        let focal = layout.radius * layout.focal_length;
        let d = layout.camera_distance;
        let tier = tier_for_scale(((d - 1.) / (focal * ppp)).to_degrees());
        let farthest = corners(rect)
            .into_iter()
            .map(|p| p.distance(layout.center))
            .fold(0., f32::max);
        let slope = farthest / focal;
        let window = if slope >= 1. / (d * d - 1.).sqrt() {
            Window::WORLD
        } else {
            // Invert perspective at the farthest corner; this spherical cap
            // encloses the entire visible rectangle, including at the poles.
            let radius = ((d * slope / (1. + slope * slope).sqrt())
                .clamp(0., 1.)
                .asin()
                - slope.atan())
            .to_degrees()
                + 0.01;
            let p = view.globe_center_latlon();
            let lon = if p.lat.abs() + radius >= 90. {
                180.
            } else {
                (radius.to_radians().sin() / p.lat.to_radians().cos())
                    .clamp(0., 1.)
                    .asin()
                    .to_degrees()
            };
            Window::around([p.lon - lon, p.lat - radius, p.lon + lon, p.lat + radius])
        };
        Self {
            tier,
            window,
            shade,
        }
    }
    pub fn local(p: &LocalProjectionParams, rect: egui::Rect, ppp: f32, shade: bool) -> Self {
        let vertical = p.ground_depth_scale * p.pitch_sin - p.ground_pitch_scale * p.pitch_cos;
        let scale =
            p.horizontal_scale.abs().max(vertical.abs()) * p.x_factor.abs().max(p.y_factor.abs());
        let tier = tier_for_scale(1. / (scale * ppp));
        let b = p.ground_bounds(rect).unwrap_or([-180., -90., 180., 90.]);
        Self {
            tier,
            window: Window::around(b),
            shade,
        }
    }
}
fn corners(r: egui::Rect) -> [egui::Pos2; 4] {
    [
        r.left_top(),
        r.right_top(),
        r.left_bottom(),
        r.right_bottom(),
    ]
}
