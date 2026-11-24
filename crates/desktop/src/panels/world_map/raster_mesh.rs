//! Geographic raster projection shared by satellite imagery and gravity overlays.
use super::globe_scene::{GlobeLayout, project_surface};
use super::local_terrain_scene::{
    LocalLayout,
    projection::{LocalProjectionParams, local_projection_params},
};
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
    let rows = ((top - bottom) / step).ceil().max(1.0) as usize;
    let cols = ((right - left) / step).ceil().max(1.0) as usize;
    let lat_verts = rows + 1;
    let lon_verts = cols + 1;
    let min_z = 1.0 / layout.camera_distance;

    // Precompute lon cos/sin once per column instead of 4x per cell.
    let mut lon_cs = Vec::with_capacity(lon_verts);
    for c in 0..lon_verts {
        let u = c as f32 / cols as f32;
        let lon = (left + (right - left) * u).to_radians();
        let (s, cs) = lon.sin_cos();
        lon_cs.push((cs, s, u));
    }

    // Project each shared grid vertex ONCE instead of 4x per cell.
    let mut vert_indices = vec![u32::MAX; lat_verts * lon_verts];
    mesh.vertices.reserve((lat_verts * lon_verts) / 2 + 64);
    mesh.indices.reserve((rows * cols / 2) * 6 + 64);

    for r in 0..lat_verts {
        let v_frac = r as f32 / rows as f32;
        let lat_deg = bottom + (top - bottom) * v_frac;
        let v_uv = 1.0 - v_frac;
        let (lat_sin, lat_cos) = lat_deg.to_radians().sin_cos();
        let row_offset = r * lon_verts;
        for (c, &(lon_cos, lon_sin, u_uv)) in lon_cs.iter().enumerate() {
            let p = project_surface(
                layout,
                view,
                GeoPoint {
                    lat: lat_deg,
                    lon: left + (right - left) * u_uv,
                },
            );
            let _ = (lat_sin, lat_cos, lon_cos, lon_sin);
            if let Some(p) = p
                && p.front_facing
                && p.depth * 2.0 - 1.0 >= min_z
            {
                let out_idx = mesh.vertices.len() as u32;
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: p.pos,
                    uv: egui::pos2(u_uv, v_uv),
                    color: egui::Color32::WHITE,
                });
                vert_indices[row_offset + c] = out_idx;
            }
        }
    }

    for r in 0..rows {
        let row_s = r * lon_verts;
        let row_n = (r + 1) * lon_verts;
        for c in 0..cols {
            let nw = vert_indices[row_n + c];
            let ne = vert_indices[row_n + c + 1];
            let se = vert_indices[row_s + c + 1];
            let sw = vert_indices[row_s + c];
            if nw != u32::MAX && ne != u32::MAX && se != u32::MAX && sw != u32::MAX {
                mesh.indices.extend_from_slice(&[nw, ne, se, nw, se, sw]);
            }
        }
    }
    mesh
}

pub(super) fn ground_projection(
    layout: &LocalLayout,
    view: &GlobeViewState,
    focus: GeoPoint,
    half: f32,
) -> LocalProjectionParams {
    let extent_y = (half * 111.32).max(1.0);
    let extent_x = (half * 111.32 * focus.lat.to_radians().cos().abs().max(0.2)).max(1.0);
    local_projection_params(layout, view, focus, extent_x, extent_y)
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
    let projection = ground_projection(layout, view, focus, half);
    let project = |lat, lon| {
        let (x, y, _) = projection.project_logical(GeoPoint { lat, lon }, 0.0);
        egui::pos2(x, y)
    };
    let Some(visible) = projection.ground_bounds(clip.expand(2.0)) else {
        return mesh;
    };
    let mid = (visible[0] + visible[2]) * 0.5;
    let shift = ((mid - (west + east) * 0.5) / 360.0).round() * 360.0;
    west += shift;
    east += shift;
    let mut visible = visible;
    if projection.pole == 0.0 {
        visible[0] = visible[0].max(focus.lon - 180.0);
        visible[2] = visible[2].min(focus.lon + 180.0);
    }
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
        // Polar latitude rings are arcs. Bound their chord error in screen points.
        let steps = if projection.pole == 0.0 {
            1
        } else {
            let radius = (90.0 - projection.pole * bottom).max(90.0 - projection.pole * top);
            let px = radius * projection.x_factor * projection.horizontal_scale;
            ((right - left).to_radians() * (px / 4.0).sqrt())
                .ceil()
                .max(1.0) as usize
        };
        for i in 0..steps {
            let l = egui::lerp(left..=right, i as f32 / steps as f32);
            let r = egui::lerp(left..=right, (i + 1) as f32 / steps as f32);
            let start = mesh.vertices.len() as u32;
            for (lat, lon) in [(top, l), (top, r), (bottom, r), (bottom, l)] {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: project(lat, lon),
                    uv: egui::pos2(
                        ((lon - (west + offset)) / (east - west)).clamp(0., 1.),
                        ((north - lat) / (north - south)).clamp(0., 1.),
                    ),
                    color: egui::Color32::WHITE,
                });
            }
            mesh.indices.extend([0, 1, 2, 0, 2, 3].map(|i| start + i));
        }
    }
    mesh
}

#[cfg(test)]
#[path = "raster_mesh_tests.rs"]
mod tests;
