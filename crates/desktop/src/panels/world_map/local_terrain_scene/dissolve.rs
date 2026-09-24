use crate::model::{GeoPoint, GlobeViewState};
use crate::theme;

use super::super::srtm_focus_cache;
use super::projection::project_local;
use super::{LocalLayout, visual_half_extent_for_zoom};

/// Each tile has 10,000 deterministically shuffled cells: one per 0.01% of
/// measured workflow completion. Only real loading progress removes cells.
/// Time affects brightness, never the completed-cell count or ordering.
pub(super) fn draw_tile_pulse_grid(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    viewport_center: GeoPoint,
    render_zoom: f32,
    radius: i32,
    time: f64,
    ready_buckets: &std::collections::HashSet<(i32, i32)>,
    progress: &std::collections::HashMap<(i32, i32), f32>,
    half_extent_override: Option<f32>,
) {
    puffin::profile_function!();
    const EDGE_BAND: f32 = 0.14; // fraction of remaining work that counts as "burning"
    const CELL_INSET: f32 = 0.10; // fractional gap between cells (10% each side)
    const OPACITY: f32 = 0.75;

    let half_extent =
        half_extent_override.unwrap_or_else(|| srtm_focus_cache::half_extent_for_zoom(render_zoom));
    let bucket_step = half_extent * 0.45;
    let visual_half = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (visual_half * km_per_deg_lon).max(1.0);
    let extent_y_km = (visual_half * km_per_deg_lat).max(1.0);

    let center_lat_b = (viewport_center.lat / bucket_step).round() as i32;
    let center_lon_b = (viewport_center.lon / bucket_step).round() as i32;
    let half = if half_extent_override.is_some() {
        half_extent
    } else {
        bucket_step * 0.5
    };

    // Brightness indicates activity without advancing completion.
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
            if !painter.clip_rect().intersects(egui::Rect::from_points(&sc)) {
                continue;
            }
            let (nw, ne, se, sw) = (sc[0], sc[1], sc[2], sc[3]);

            let removed = completed_cells(progress.get(&(lat_b, lon_b)).copied().unwrap_or(0.0));
            if removed == GRID * GRID {
                continue;
            }
            let ranks = cell_ranks(tile_hash(lat_b, lon_b));

            for (cell, rank) in ranks.enumerate() {
                let row = cell / GRID;
                let col = cell % GRID;
                if rank < removed {
                    continue; // this cell has dissolved
                }

                // 1.0 = far from dissolving, 0.0 = about to vanish
                let edge =
                    ((rank - removed) as f32 / (GRID * GRID) as f32 / EDGE_BAND).clamp(0.0, 1.0);

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
                let alpha = (lerp_f32(8.0, 80.0, 1.0 - edge) * breath * OPACITY) as u8;
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
                    let fa = (t * 40.0 * breath * OPACITY) as u8;
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

const GRID: usize = 100;

fn completed_cells(progress: f32) -> usize {
    if !progress.is_finite() {
        return 0;
    }
    (progress.clamp(0.0, 1.0) * (GRID * GRID) as f32).floor() as usize
}

/// A permutation gives exactly N dissolved cells, unlike random float
/// thresholds which only approximate a percentage and can finish early.
fn cell_ranks(seed: u64) -> impl Iterator<Item = usize> {
    // Sort only once, rather than sorting 10,000 cells for every tile/frame.
    // Rotating the ranks keeps each tile's pattern stable and the exact count
    // of removed cells intact without allocating another per-tile array.
    static RANKS: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();
    let ranks = RANKS.get_or_init(|| {
        let mut order: Vec<usize> = (0..GRID * GRID).collect();
        order.sort_unstable_by_key(|&i| {
            let mut x = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^ (x >> 31)
        });
        let mut ranks = vec![0; GRID * GRID];
        for (rank, index) in order.into_iter().enumerate() {
            ranks[index] = rank;
        }
        ranks
    });
    let offset = (seed % (GRID * GRID) as u64) as usize;
    ranks
        .iter()
        .map(move |rank| (rank + offset) % (GRID * GRID))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painted_cells_hold_across_the_old_timer_cycle_and_clear_on_readiness() {
        let paint = |time, fraction, ready| {
            let ctx = egui::Context::default();
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
                let painter = ctx.layer_painter(egui::LayerId::background());
                let focus = GeoPoint { lat: 0.0, lon: 0.0 };
                let view = GlobeViewState::from_focus(focus);
                let ready = if ready {
                    std::collections::HashSet::from([(0, 0)])
                } else {
                    std::collections::HashSet::new()
                };
                draw_tile_pulse_grid(
                    &painter,
                    &super::super::layout(rect),
                    &view,
                    focus,
                    25.0,
                    0,
                    time,
                    &ready,
                    &std::collections::HashMap::from([((0, 0), fraction)]),
                    None,
                );
            });
            output
                .shapes
                .into_iter()
                .find_map(|shape| match shape.shape {
                    egui::Shape::Mesh(mesh) => {
                        Some(mesh.vertices.iter().map(|v| v.pos).collect::<Vec<_>>())
                    }
                    _ => None,
                })
                .unwrap_or_default()
        };
        let stalled = paint(0.0, 0.5, false);
        assert_eq!(stalled.len(), 5_000 * 4);
        assert_eq!(paint(6.9, 0.5, false), stalled);
        assert_eq!(paint(7.1, 0.5, false), stalled);
        assert_eq!(paint(100.0, 0.5, false), stalled);
        assert_eq!(paint(100.0, 0.75, false).len(), 2_500 * 4);
        assert_eq!(paint(100.0, 0.99, false).len(), 100 * 4);
        assert!(paint(100.0, 1.0, false).is_empty());
        assert!(paint(100.0, 0.0, true).is_empty());
    }

    #[test]
    fn dissolve_tracks_completed_work_exactly_without_reappearing() {
        let ranks: Vec<_> = cell_ranks(tile_hash(5467, -16911)).collect();
        let mut previous = std::collections::HashSet::new();
        for progress in [0.0, 0.25, 0.50, 0.75, 0.99, 1.0] {
            let gone: std::collections::HashSet<_> = ranks
                .iter()
                .enumerate()
                .filter_map(|(cell, &rank)| (rank < completed_cells(progress)).then_some(cell))
                .collect();
            assert_eq!(gone.len(), completed_cells(progress));
            assert!(previous.is_subset(&gone));
            previous = gone;
        }
        assert_eq!(previous.len(), 10_000);
        assert_eq!(
            cell_ranks(tile_hash(5467, -16911)).collect::<Vec<_>>(),
            ranks
        );
        assert_ne!(
            cell_ranks(tile_hash(5467, -16910)).collect::<Vec<_>>(),
            ranks
        );
    }

    #[test]
    fn queued_unknown_and_stalled_work_do_not_claim_completion() {
        assert_eq!(completed_cells(0.0), 0);
        assert_eq!(completed_cells(f32::NAN), 0);
        assert_eq!(completed_cells(f32::INFINITY), 0);
        assert_eq!(completed_cells(-1.0), 0);
        assert_eq!(completed_cells(0.99), 9_900);
        assert_eq!(completed_cells(5.0), 10_000);
    }
}

#[inline]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).clamp(0.0, 255.0) as u8
}

#[inline]
fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
