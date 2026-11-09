//! Geographic raster projection shared by satellite imagery and gravity overlays.
use super::globe_scene::{GlobeLayout, project_surface};
use super::local_terrain_scene::{LocalLayout, projection::local_projection_params};
use crate::model::{GeoPoint, GlobeViewState};

pub(super) fn globe(
    layout: &GlobeLayout,
    view: &GlobeViewState,
    texture: egui::TextureId,
    bounds: [f32; 4],
    detailed: bool,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::with_texture(texture);
    let [left, bottom, right, top] = bounds;
    let scale = layout.radius * layout.focal_length / (layout.camera_distance - 1.0);
    let step = if detailed {
        (0.5 / scale).sqrt().to_degrees().clamp(0.25, 2.0)
    } else {
        2.0
    };
    let rows = ((top - bottom) / step).ceil() as usize;
    let cols = ((right - left) / step).ceil() as usize;
    // Both textures use their own exact geographic edges and UV map.
    for row in 0..rows {
        let south = bottom + (top - bottom) * row as f32 / rows as f32;
        let north = bottom + (top - bottom) * (row + 1) as f32 / rows as f32;
        for col in 0..cols {
            let west = left + (right - left) * col as f32 / cols as f32;
            let east = left + (right - left) * (col + 1) as f32 / cols as f32;
            let corners = [(north, west), (north, east), (south, east), (south, west)];
            let projected =
                corners.map(|(lat, lon)| project_surface(layout, view, GeoPoint { lat, lon }));
            // A perspective camera sees only z >= 1/d on the unit sphere;
            // hemisphere culling alone would paint hidden surface near the limb.
            if !projected.iter().all(|p| {
                p.is_some_and(|p| {
                    p.front_facing && p.depth * 2.0 - 1.0 >= 1.0 / layout.camera_distance
                })
            }) {
                continue;
            }
            let index = mesh.vertices.len() as u32;
            for (i, (lat, lon)) in corners.into_iter().enumerate() {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: projected[i].unwrap().pos,
                    uv: egui::pos2((lon - left) / (right - left), (top - lat) / (top - bottom)),
                    color: egui::Color32::WHITE,
                });
            }
            mesh.indices.extend_from_slice(&[
                index,
                index + 1,
                index + 2,
                index,
                index + 2,
                index + 3,
            ]);
        }
    }
    mesh
}

pub(super) fn local(
    layout: &LocalLayout,
    view: &GlobeViewState,
    texture: egui::TextureId,
    bounds: [f32; 4],
    focus: GeoPoint,
    half: f32,
    clip: egui::Rect,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::with_texture(texture);
    let [mut west, south, mut east, north] = bounds;
    // Put an unwrapped source such as Alaska beside the current focus.
    let shift = ((focus.lon - (west + east) * 0.5) / 360.0).round() * 360.0;
    west += shift;
    east += shift;
    let extent_y = (half * 111.32).max(1.0);
    let extent_x = (half * 111.32 * focus.lat.to_radians().cos().abs().max(0.2)).max(1.0);
    let projection = local_projection_params(layout, view, focus, extent_x, extent_y);
    let project = |lat, lon| {
        let (x, y, _) = projection.project_logical(GeoPoint { lat, lon }, 0.0);
        egui::pos2(x, y)
    };
    // At sea level the local transform is affine. Invert the actual clip
    // rectangle before drawing, so deep zoom cannot cull every coarse vertex
    // of a large region, or introduce extreme offscreen coordinates.
    let origin = project(focus.lat, focus.lon);
    let u = project(focus.lat, focus.lon + 1.0) - origin;
    let v = project(focus.lat + 1.0, focus.lon) - origin;
    let det = u.x * v.y - u.y * v.x;
    if !det.is_finite() || det.abs() < 1e-6 {
        return mesh;
    }
    let mut visible = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    let clip = clip.expand(2.0);
    for corner in [
        clip.left_top(),
        clip.right_top(),
        clip.right_bottom(),
        clip.left_bottom(),
    ] {
        let delta = corner - origin;
        let lon = focus.lon + (delta.x * v.y - delta.y * v.x) / det;
        let lat = focus.lat + (u.x * delta.y - u.y * delta.x) / det;
        visible[0] = visible[0].min(lon);
        visible[1] = visible[1].min(lat);
        visible[2] = visible[2].max(lon);
        visible[3] = visible[3].max(lat);
    }
    // Near edge-on pitch the clip rectangle can span multiple revolutions.
    // Show each longitude only once in the plane centered on the observer.
    visible[0] = visible[0].max(focus.lon - 180.0);
    visible[2] = visible[2].min(focus.lon + 180.0);
    let bottom = south.max(visible[1]);
    let top = north.min(visible[3]);
    if bottom >= top {
        return mesh;
    }
    // A full-width image can have both UV edges inside one dateline viewport.
    for offset in [-360.0, 0.0, 360.0] {
        let left = (west + offset).max(visible[0]);
        let right = (east + offset).min(visible[2]);
        if left >= right {
            continue;
        }
        let start = mesh.vertices.len() as u32;
        for (lat, lon) in [(top, left), (top, right), (bottom, right), (bottom, left)] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: project(lat, lon),
                uv: egui::pos2(
                    (lon - (west + offset)) / (east - west),
                    (north - lat) / (north - south),
                ),
                color: egui::Color32::WHITE,
            });
        }
        mesh.indices.extend([0, 1, 2, 0, 2, 3].map(|i| start + i));
    }
    mesh
}

#[cfg(test)]
#[path = "raster_mesh_tests.rs"]
mod tests;
