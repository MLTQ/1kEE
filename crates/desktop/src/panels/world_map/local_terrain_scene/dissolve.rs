use crate::model::{GeoPoint, GlobeViewState};
use crate::theme;

use super::super::srtm_focus_cache;
use super::projection::project_local;
use super::{LocalLayout, visual_half_extent_for_zoom};

/// Draws a pulsing glow over every tile footprint in the expected load grid.
/// Each tile is projected as a quadrilateral matching its actual geo-extent so
/// the placeholder fills exactly the area that will be covered by contour lines
/// once the tile finishes building.  Tiles with loaded geometry naturally cover
/// their pulse; pending tiles keep glowing until data arrives.
/// Spectral dissolve: each pending tile is subdivided into an 8×8 grid of
/// cells.  A time-based "cursor" sweeps 0→1 over DISSOLVE_CYCLE seconds;
/// each cell has a deterministic random threshold and disappears when the
/// cursor passes it.  Cells near their threshold glow with `hot_color` and
/// get a chromatic-aberration fringe drawn in the theme pair colors.
pub(super) fn draw_tile_pulse_grid(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    viewport_center: GeoPoint,
    render_zoom: f32,
    radius: i32,
    time: f64,
    ready_buckets: &std::collections::HashSet<(i32, i32)>,
    half_extent_override: Option<f32>,
) {
    const GRID: usize = 50; // 50×50 = 2 500 cells per tile
    const DISSOLVE_CYCLE: f64 = 7.0; // seconds for one full sweep
    const EDGE_BAND: f32 = 0.14; // fraction of cycle that counts as "burning"
    const CELL_INSET: f32 = 0.10; // fractional gap between cells (10% each side)

    let half_extent = half_extent_override
        .unwrap_or_else(|| srtm_focus_cache::half_extent_for_zoom(render_zoom));
    let bucket_step = half_extent * 0.45;
    let visual_half = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (visual_half * km_per_deg_lon).max(1.0);
    let extent_y_km = (visual_half * km_per_deg_lat).max(1.0);

    let center_lat_b = (viewport_center.lat / bucket_step).round() as i32;
    let center_lon_b = (viewport_center.lon / bucket_step).round() as i32;
    let half = half_extent;

    // Dissolve cursor: 0 (all cells visible) → 1 (all gone), then resets.
    let cursor = ((time % DISSOLVE_CYCLE) / DISSOLVE_CYCLE) as f32;

    // Gentle global breath layered on top so nothing ever feels static.
    let breath = ((time as f32 * std::f32::consts::TAU / 4.5).sin() * 0.5 + 0.5) * 0.35 + 0.65;

    // Theme colours — hot_color for the burning edge, contour_color for the
    // calm interior.  The CA fringe uses both so it themes automatically.
    let [cr, cg, cb, _] = theme::contour_color().to_array();
    let [hr, hg, hb, _] = theme::hot_color().to_array();

    // One mesh for the main cells, two for the chromatic fringe ghosts.
    let mut mesh = egui::Mesh::default();
    let mut mesh_hot = egui::Mesh::default(); // shifted toward hot_color
    let mut mesh_cnt = egui::Mesh::default(); // shifted toward contour_color

    for dlat in -radius..=radius {
        for dlon in -radius..=radius {
            let lat_b = center_lat_b + dlat;
            let lon_b = center_lon_b + dlon;
            if ready_buckets.contains(&(lat_b, lon_b)) {
                continue;
            }

            let tile_lat = (lat_b as f32 * bucket_step).clamp(-89.9, 89.9);
            let tile_lon = lon_b as f32 * bucket_step;

            // Project the 4 geo corners → screen space (NW, NE, SE, SW).
            let geo_corners = [
                GeoPoint {
                    lat: tile_lat + half,
                    lon: tile_lon - half,
                },
                GeoPoint {
                    lat: tile_lat + half,
                    lon: tile_lon + half,
                },
                GeoPoint {
                    lat: tile_lat - half,
                    lon: tile_lon + half,
                },
                GeoPoint {
                    lat: tile_lat - half,
                    lon: tile_lon - half,
                },
            ];
            let sc: Vec<egui::Pos2> = geo_corners
                .iter()
                .filter_map(|&c| {
                    project_local(
                        layout,
                        view,
                        viewport_center,
                        c,
                        0.0,
                        extent_x_km,
                        extent_y_km,
                    )
                })
                .map(|p| p.pos)
                .collect();
            if sc.len() < 4 {
                continue;
            }
            let (nw, ne, se, sw) = (sc[0], sc[1], sc[2], sc[3]);

            let seed = tile_hash(lat_b, lon_b);

            for row in 0..GRID {
                for col in 0..GRID {
                    let threshold = cell_rand(seed, row, col);
                    if threshold < cursor {
                        continue; // this cell has dissolved
                    }

                    // 1.0 = far from dissolving, 0.0 = about to vanish
                    let edge = ((threshold - cursor) / EDGE_BAND).clamp(0.0, 1.0);

                    // Bilinear sub-quad with a tiny inset gap.
                    let n = GRID as f32;
                    let u0 = col as f32 / n + CELL_INSET / n;
                    let u1 = (col as f32 + 1.0) / n - CELL_INSET / n;
                    let v0 = row as f32 / n + CELL_INSET / n;
                    let v1 = (row as f32 + 1.0) / n - CELL_INSET / n;

                    let p_nw = bilerp(nw, ne, sw, se, u0, v0);
                    let p_ne = bilerp(nw, ne, sw, se, u1, v0);
                    let p_se = bilerp(nw, ne, sw, se, u1, v1);
                    let p_sw = bilerp(nw, ne, sw, se, u0, v1);

                    // Mix contour→hot as cell approaches its threshold.
                    let mix = (1.0 - edge).powf(1.8);
                    let r = lerp_u8(cr, hr, mix);
                    let g = lerp_u8(cg, hg, mix);
                    let b = lerp_u8(cb, hb, mix);
                    let alpha = (lerp_f32(8.0, 80.0, 1.0 - edge) * breath) as u8;
                    quad(
                        &mut mesh,
                        p_nw,
                        p_ne,
                        p_se,
                        p_sw,
                        egui::Color32::from_rgba_unmultiplied(r, g, b, alpha),
                    );

                    // Chromatic-aberration fringe on burning-edge cells.
                    if edge < 0.4 {
                        let t = 1.0 - edge / 0.4; // 0→1 as cell nears threshold
                        let fa = (t * 40.0 * breath) as u8;
                        let offset = egui::Vec2::new(t * 1.8, 0.0);

                        // Hot ghost shifted one way
                        quad(
                            &mut mesh_hot,
                            p_nw + offset,
                            p_ne + offset,
                            p_se + offset,
                            p_sw + offset,
                            egui::Color32::from_rgba_unmultiplied(hr, hg, hb, fa),
                        );
                        // Contour ghost shifted the other way
                        quad(
                            &mut mesh_cnt,
                            p_nw - offset,
                            p_ne - offset,
                            p_se - offset,
                            p_sw - offset,
                            egui::Color32::from_rgba_unmultiplied(cr, cg, cb, fa),
                        );
                    }
                }
            }
        }
    }

    if !mesh.vertices.is_empty() {
        painter.add(egui::Shape::mesh(mesh));
    }
    if !mesh_hot.vertices.is_empty() {
        painter.add(egui::Shape::mesh(mesh_hot));
    }
    if !mesh_cnt.vertices.is_empty() {
        painter.add(egui::Shape::mesh(mesh_cnt));
    }
}

pub(super) fn draw_frame(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_stroke(
        rect.shrink(6.0),
        12.0,
        egui::Stroke::new(0.7, theme::topo_color().gamma_multiply(0.45)),
        egui::StrokeKind::Outside,
    );

    for &(x, y, x_dir, y_dir) in &[
        (rect.left() + 18.0, rect.top() + 18.0, 28.0, 16.0),
        (rect.right() - 18.0, rect.top() + 18.0, -28.0, 16.0),
        (rect.left() + 18.0, rect.bottom() - 18.0, 28.0, -16.0),
        (rect.right() - 18.0, rect.bottom() - 18.0, -28.0, -16.0),
    ] {
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2(x + x_dir, y)],
            egui::Stroke::new(1.0, theme::topo_color()),
        );
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2(x, y + y_dir)],
            egui::Stroke::new(1.0, theme::topo_color()),
        );
    }
}

// ── tile-dissolve helpers ──────────────────────────────────────────────────

/// Bilinear interpolation across a screen-space quad.
/// Corners: NW (u=0,v=0), NE (u=1,v=0), SW (u=0,v=1), SE (u=1,v=1).
#[inline]
fn bilerp(
    nw: egui::Pos2,
    ne: egui::Pos2,
    sw: egui::Pos2,
    se: egui::Pos2,
    u: f32,
    v: f32,
) -> egui::Pos2 {
    nw.lerp(ne, u).lerp(sw.lerp(se, u), v)
}

/// Append a solid-colour quad (two triangles) to `mesh`.
#[inline]
fn quad(
    mesh: &mut egui::Mesh,
    nw: egui::Pos2,
    ne: egui::Pos2,
    se: egui::Pos2,
    sw: egui::Pos2,
    color: egui::Color32,
) {
    let i = mesh.vertices.len() as u32;
    mesh.colored_vertex(nw, color);
    mesh.colored_vertex(ne, color);
    mesh.colored_vertex(se, color);
    mesh.colored_vertex(sw, color);
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
}

/// Stable per-tile seed from bucket coordinates.
#[inline]
fn tile_hash(lat_b: i32, lon_b: i32) -> u64 {
    let a = (lat_b as u64) & 0xFFFF_FFFF;
    let b = (lon_b as u64) & 0xFFFF_FFFF;
    a.wrapping_mul(2_654_435_761)
        .wrapping_add(b.wrapping_mul(2_246_822_519))
        .wrapping_mul(6_364_136_223_846_793_005)
}

/// Deterministic float in [0, 1) for a given tile seed + (row, col).
///
/// Uses a splitmix64-style finalizer: row and col are mixed into the seed
/// with different primes *before* the avalanche pass, so adjacent cells
/// produce completely uncorrelated values rather than an arithmetic sequence.
#[inline]
fn cell_rand(seed: u64, row: usize, col: usize) -> f32 {
    let mut x = seed
        .wrapping_add((row as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
        .wrapping_add((col as u64).wrapping_mul(0x6c62_272e_07bb_0142));
    // splitmix64 avalanche
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

#[inline]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).clamp(0.0, 255.0) as u8
}

#[inline]
fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
