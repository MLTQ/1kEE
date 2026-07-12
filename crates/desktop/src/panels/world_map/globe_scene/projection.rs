use crate::model::{GeoPoint, GlobeViewState};

use super::terrain_field;
use super::{GlobeLayout, ProjectedPoint};

/// The GPU globe backdrop ray-intersects this exact radius. Terrain affects
/// shading there, not the sphere's geometric surface.
const UNIT_SPHERE_RADIUS: f32 = 1.0;
/// Retain the legacy terrain-aware projection floor for generic overlays.
const MIN_TERRAIN_RADIUS: f32 = 0.82;

/// Frame-invariant inputs to the globe rotation+perspective transform. The
/// `sin/cos` of yaw and pitch are identical for every point of a frame, so we
/// derive them once per key instead of once per projected point.
#[derive(Clone, Copy, PartialEq)]
struct XformKey {
    yaw: f32,
    pitch: f32,
    camera_distance: f32,
    radius: f32,
    focal_length: f32,
    center_x: f32,
    center_y: f32,
}

struct GlobeXform {
    key: XformKey,
    yaw_cos: f32,
    yaw_sin: f32,
    pitch_cos: f32,
    pitch_sin: f32,
    camera_distance: f32,
    radius_focal: f32,
    center: egui::Pos2,
}

impl GlobeXform {
    fn new(key: XformKey) -> Self {
        GlobeXform {
            key,
            yaw_cos: key.yaw.cos(),
            yaw_sin: key.yaw.sin(),
            pitch_cos: key.pitch.cos(),
            pitch_sin: key.pitch.sin(),
            camera_distance: key.camera_distance,
            radius_focal: key.radius * key.focal_length,
            center: egui::pos2(key.center_x, key.center_y),
        }
    }

    /// Apply yaw/pitch rotation and perspective projection to a pre-rotation
    /// sphere-space point `(x, y, z)`.
    #[inline]
    fn apply(&self, x: f32, y: f32, z: f32) -> Option<ProjectedPoint> {
        let x_yaw = x * self.yaw_cos + z * self.yaw_sin;
        let z_yaw = -x * self.yaw_sin + z * self.yaw_cos;
        let x = x_yaw;
        let z = z_yaw;

        let y_pitch = y * self.pitch_cos - z * self.pitch_sin;
        let z_pitch = y * self.pitch_sin + z * self.pitch_cos;
        let y = y_pitch;
        let z = z_pitch;

        let depth = self.camera_distance - z;
        if depth <= 0.05 {
            return None;
        }

        let perspective = self.radius_focal / depth;
        let pos = egui::pos2(
            self.center.x - x * perspective,
            self.center.y - y * perspective,
        );
        Some(ProjectedPoint {
            pos,
            depth: ((z + 1.0) * 0.5).clamp(0.0, 1.0),
            front_facing: z >= 0.0,
        })
    }
}

/// Per-thread cache of the rotation transform so the per-point projection
/// functions below don't each recompute four transcendentals per point. Used
/// from rayon workers too, so it's thread-local rather than a shared lock.
fn with_xform<R>(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    f: impl FnOnce(&GlobeXform) -> R,
) -> R {
    let key = XformKey {
        yaw: view.yaw,
        pitch: view.pitch,
        camera_distance: layout.camera_distance,
        radius: layout.radius,
        focal_length: layout.focal_length,
        center_x: layout.center.x,
        center_y: layout.center.y,
    };
    thread_local! {
        static CACHE: std::cell::RefCell<Option<GlobeXform>> = const { std::cell::RefCell::new(None) };
    }
    CACHE.with(|c| {
        let mut slot = c.borrow_mut();
        if slot.as_ref().map(|x| x.key) != Some(key) {
            *slot = Some(GlobeXform::new(key));
        }
        f(slot.as_ref().unwrap())
    })
}

fn project_geo_at_radius(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
    radius: f32,
) -> Option<ProjectedPoint> {
    let lat = point.lat.to_radians();
    let lon = point.lon.to_radians();
    let lat_cos = lat.cos();
    let x = radius * lat_cos * lon.cos();
    let y = radius * lat.sin();
    let z = radius * lat_cos * lon.sin();

    with_xform(layout, view, |xf| xf.apply(x, y, z))
}

fn terrain_radius(point: GeoPoint, altitude_scale: f32, extra_radius: f32) -> f32 {
    let elevation_signal = terrain_field::elevation(point) / 1.6;
    let signed_elevation = elevation_signal.mul_add(2.0, -1.0);
    (UNIT_SPHERE_RADIUS + signed_elevation * altitude_scale + extra_radius).max(MIN_TERRAIN_RADIUS)
}

fn unit_surface_radius(extra_radius: f32) -> f32 {
    UNIT_SPHERE_RADIUS + extra_radius.max(0.0)
}

/// Project an interactive indicator base onto the visible globe surface.
///
/// Unlike generic terrain-aware projections, this deliberately uses the same
/// unit radius as the GPU backdrop. The shader shades terrain without changing
/// sphere geometry, so a negative synthetic terrain displacement would make a
/// marker appear inside the globe.
pub(super) fn project_geo_unit_surface(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
) -> Option<ProjectedPoint> {
    project_geo_at_radius(layout, view, point, UNIT_SPHERE_RADIUS)
}

/// Project a point strictly outward from the unit-sphere surface.
///
/// Event and replay beam tips use this with their existing visual beam height,
/// guaranteeing that the tip remains above its surface-projected base even
/// over terrain signals that would otherwise be negative.
pub(super) fn project_geo_unit_surface_elevated(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
    extra_radius: f32,
) -> Option<ProjectedPoint> {
    project_geo_at_radius(layout, view, point, unit_surface_radius(extra_radius))
}

/// Like `project_geo` but adds `extra_radius` (in globe-unit fractions) on
/// top of the terrain-based elevation.  Used to project a beam-tip point
/// directly above a geographic location so that the resulting screen-space
/// vector gives a perspective-correct beam direction: very short when the
/// event faces the camera, full-length when it is on the limb.
#[allow(dead_code)]
pub(super) fn project_geo_elevated(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
    altitude_scale: f32,
    extra_radius: f32,
) -> Option<ProjectedPoint> {
    project_geo_at_radius(
        layout,
        view,
        point,
        terrain_radius(point, altitude_scale, extra_radius),
    )
}

pub fn project_geo(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
    altitude_scale: f32,
) -> Option<ProjectedPoint> {
    project_geo_at_radius(
        layout,
        view,
        point,
        terrain_radius(point, altitude_scale, 0.0),
    )
}

/// Like `project_geo` but skips `terrain_field::elevation` — uses a constant
/// radius offset instead.  Eliminates 6 `exp()` calls per point; the ±1.5%
/// terrain-driven radius variation is imperceptible on thin line strokes.
fn project_geo_flat(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    point: GeoPoint,
    radius_offset: f32,
) -> Option<ProjectedPoint> {
    project_geo_at_radius(layout, view, point, UNIT_SPHERE_RADIUS + radius_offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_projection_inputs() -> (GlobeLayout, GlobeViewState, GeoPoint) {
        let view = GlobeViewState::from_focus(GeoPoint { lat: 0.0, lon: 0.0 });
        let layout = GlobeLayout {
            center: egui::pos2(400.0, 300.0),
            radius: 180.0,
            focal_length: 2.2,
            camera_distance: 3.0,
        };
        // Visible and off-axis, so radial movement is observable in screen space.
        let point = GeoPoint {
            lat: 0.0,
            lon: 20.0,
        };
        (layout, view, point)
    }

    #[test]
    fn unit_surface_base_matches_the_generic_zero_altitude_projection() {
        let (layout, view, point) = test_projection_inputs();

        let surface = project_geo_unit_surface(&layout, &view, point).expect("surface point");
        let generic = project_geo(&layout, &view, point, 0.0).expect("generic point");

        assert_eq!(surface.pos, generic.pos);
        assert_eq!(surface.front_facing, generic.front_facing);
    }

    #[test]
    fn elevated_unit_surface_helper_never_moves_a_beam_tip_inward() {
        let (layout, view, point) = test_projection_inputs();

        let surface = project_geo_unit_surface(&layout, &view, point).expect("surface point");
        let raised =
            project_geo_unit_surface_elevated(&layout, &view, point, 0.08).expect("raised point");
        let clamped =
            project_geo_unit_surface_elevated(&layout, &view, point, -0.08).expect("clamped point");

        assert!(raised.pos.distance(layout.center) > surface.pos.distance(layout.center));
        assert_eq!(clamped.pos, surface.pos);
    }
}

/// Draw a geographic polyline on the globe, clipping at the horizon.
///
/// Uses a flat (constant-radius) projection — no terrain field — for
/// performance. Back-facing segments are skipped entirely (they are
/// nearly invisible at the alpha values used and were the source of
/// "laser" artifacts when single orphan points straddled the horizon).
pub(super) fn draw_geo_path(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    path: &[GeoPoint],
    altitude_scale: f32,
    front_color: egui::Color32,
    _backface_alpha: f32,
) {
    let stroke = egui::Stroke::new(1.15, front_color.gamma_multiply(0.92));
    let mut segment: Vec<egui::Pos2> = Vec::new();

    for point in path {
        match project_geo_flat(layout, view, *point, altitude_scale) {
            Some(p) if p.front_facing => segment.push(p.pos),
            _ => {
                // Back-facing or behind near-plane — break the current segment.
                // Always clear (even a single-point orphan) to prevent the orphan
                // being joined to the next visible run, which produced "laser" lines.
                if segment.len() >= 2 {
                    painter.add(egui::Shape::line(std::mem::take(&mut segment), stroke));
                } else {
                    segment.clear();
                }
            }
        }
    }

    if segment.len() >= 2 {
        painter.add(egui::Shape::line(segment, stroke));
    }
}
