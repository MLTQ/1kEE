use super::GlobeLayout;
use super::camera::GlobeLod;
use super::contour_asset;
use super::gebco_depth_fill;
use super::projection::{draw_geo_path, project_geo};
use crate::model::{GeoJsonFeature, GeoJsonGeometry, GeoJsonLayer, GeoPoint, GlobeViewState};
use crate::theme;

// ── Lunar feature labels ──────────────────────────────────────────────────────
// Major maria and craters with lat/lon in degrees (IAU selenographic coordinates).
const LUNAR_FEATURES: &[(&str, f32, f32)] = &[
    ("Oceanus Procellarum", 18.4, -57.4),
    ("Mare Imbrium", 32.8, -15.6),
    ("Mare Tranquillitatis", 8.5, 31.4),
    ("Mare Serenitatis", 28.0, 17.5),
    ("Mare Crisium", 17.0, 59.1),
    ("Mare Nubium", -21.3, -16.6),
    ("Mare Fecunditatis", -4.5, 51.3),
];

const MARS_FEATURES: &[(&str, f32, f32)] = &[
    ("Olympus Mons", 18.65, -133.8),
    ("Valles Marineris", -13.9, -59.2),
    ("Hellas Planitia", -42.7, 70.0),
    ("Argyre Planitia", -49.7, -43.0),
    ("Elysium Mons", 25.0, 147.2),
];

pub(super) fn draw_global_coastlines(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    selected_root: Option<&std::path::Path>,
) {
    puffin::profile_function!();
    let Some(coastlines) =
        contour_asset::load_global_coastlines(selected_root, view.zoom, painter.ctx().clone())
    else {
        return;
    };

    // Thin white line — same weight as topo contours but white to distinguish
    // land/sea boundary.
    let coast_color = egui::Color32::from_rgba_premultiplied(210, 220, 255, 90);
    for coastline in coastlines.iter() {
        draw_geo_path(
            painter,
            layout,
            view,
            &coastline.points,
            0.015,
            coast_color,
            0.04,
        );
    }
}

pub(super) fn draw_global_bathymetry(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    selected_root: Option<&std::path::Path>,
) {
    puffin::profile_function!();
    // ── Layer 1: depth-fill texture mapped onto the sphere ───────────────────
    //
    // Strategy: upload a 1440×720 (0.25°/px) depth ColorImage as an egui
    // texture (land pixels = TRANSPARENT, ocean = depth colour).  Then build
    // a 2° UV-mapped sphere mesh each frame.  GPU bilinear interpolation
    // (`TextureOptions::LINEAR`) smoothly blends between the 0.25° texels,
    // so the fill follows actual bathymetry contours with no rectangular grid
    // artefacts, and land shows as the dark globe background because those
    // pixels are transparent in the texture.
    if let Some(tex_id) = gebco_depth_fill::ensure_texture(painter.ctx(), selected_root) {
        // 2° mesh: 180×90 = 16 200 cells, ~8 100 front-facing.  Coarse mesh
        // is fine because the texture provides sub-pixel depth detail.
        const STEP: f32 = 2.0;
        const HALF: f32 = STEP / 2.0;

        let mut mesh = egui::epaint::Mesh::default();
        mesh.texture_id = tex_id;

        let mut lat = -90.0_f32 + STEP;
        while lat <= 90.0 {
            let mut lon = -180.0_f32 + STEP;
            while lon <= 180.0 {
                // Four corners of this 2°×2° cell, ordered CCW on the sphere.
                let corners: [(f32, f32); 4] = [
                    (lat + HALF, lon - HALF), // NW
                    (lat + HALF, lon + HALF), // NE
                    (lat - HALF, lon + HALF), // SE
                    (lat - HALF, lon - HALF), // SW
                ];

                // UV: equirectangular — u=0 at 180°W, v=0 at 90°N.
                let uvs: [(f32, f32); 4] =
                    corners.map(|(clat, clon)| ((clon + 180.0) / 360.0, (90.0 - clat) / 180.0));

                // Project all four corners; skip if any is back-facing.
                let mut positions = [egui::Pos2::ZERO; 4];
                let mut ok = true;
                for (k, &(clat, clon)) in corners.iter().enumerate() {
                    match project_geo(
                        layout,
                        view,
                        GeoPoint {
                            lat: clat,
                            lon: clon,
                        },
                        0.0,
                    ) {
                        Some(p) if p.front_facing => positions[k] = p.pos,
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    lon += STEP;
                    continue;
                }

                let i = mesh.vertices.len() as u32;
                for k in 0..4 {
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: positions[k],
                        uv: egui::pos2(uvs[k].0, uvs[k].1),
                        color: egui::Color32::WHITE, // texture carries the colour
                    });
                }
                mesh.indices
                    .extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
                lon += STEP;
            }
            lat += STEP;
        }

        if !mesh.vertices.is_empty() {
            painter.add(egui::Shape::mesh(mesh));
        }
    }

    // ── Layer 2: isobath contour lines on top ────────────────────────────────
    let Some(bathy) =
        contour_asset::load_global_bathymetry(selected_root, view.zoom, painter.ctx().clone())
    else {
        return;
    };

    for contour in bathy.iter() {
        let depth_norm = (-contour.elevation_m / 11_000.0_f32).clamp(0.0, 1.0);
        let major = ((-contour.elevation_m.round() as i32) % 1_000) < 50;
        let base_a = if major { 0.38_f32 } else { 0.16_f32 };
        let a = (base_a * (0.5 + depth_norm * 0.5) * 255.0) as u8;
        let r = (25.0 * (1.0 - depth_norm * 0.8)) as u8;
        let g = (70.0 * (1.0 - depth_norm * 0.6)) as u8;
        let b = (175 + (40.0 * depth_norm) as u8).min(255);
        let color = egui::Color32::from_rgba_premultiplied(r, g, b, a);
        draw_geo_path(painter, layout, view, &contour.points, 0.01, color, 0.03);
    }
}

#[allow(dead_code)]
#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ── GeoJSON overlay ───────────────────────────────────────────────────────────

/// Draw all visible GeoJSON layers onto the globe.
/// Lines and polygon rings are projected via `draw_geo_path`; points become
/// filled circles; labels float above their anchor position.
pub(super) fn draw_geojson_layers(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    layers: &[GeoJsonLayer],
) {
    puffin::profile_function!();
    for layer in layers {
        if !layer.visible {
            continue;
        }
        let [r, g, b, a] = layer.color;
        let color = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
        for feature in &layer.features {
            draw_geojson_feature(painter, layout, view, &feature.geometry, color);
            draw_feature_label(painter, layout, view, feature, color);
        }
    }
}

fn draw_geojson_feature(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    geometry: &GeoJsonGeometry,
    color: egui::Color32,
) {
    match geometry {
        GeoJsonGeometry::Point(pt) => {
            if let Some(proj) = project_geo(layout, view, *pt, 0.0) {
                if proj.front_facing {
                    painter.circle_filled(proj.pos, 3.5, color);
                    painter.circle_stroke(
                        proj.pos,
                        6.0,
                        egui::Stroke::new(1.0, color.gamma_multiply(0.4)),
                    );
                }
            }
        }
        GeoJsonGeometry::LineString(pts) => {
            draw_geo_path(painter, layout, view, pts, 0.012, color, 0.25);
        }
        GeoJsonGeometry::MultiLineString(lines) => {
            for line in lines {
                draw_geo_path(painter, layout, view, line, 0.012, color, 0.25);
            }
        }
        GeoJsonGeometry::Polygon(rings) => {
            for ring in rings {
                draw_geo_path(painter, layout, view, ring, 0.012, color, 0.25);
            }
        }
        GeoJsonGeometry::MultiPolygon(polys) => {
            for poly in polys {
                for ring in poly {
                    draw_geo_path(painter, layout, view, ring, 0.012, color, 0.25);
                }
            }
        }
    }
}

fn draw_feature_label(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    feature: &GeoJsonFeature,
    color: egui::Color32,
) {
    let Some(label) = &feature.label else { return };
    let anchor_pt = label_anchor(&feature.geometry);
    let Some(pt) = anchor_pt else { return };
    let Some(proj) = project_geo(layout, view, pt, 0.0) else {
        return;
    };
    if !proj.front_facing {
        return;
    }
    let text_pos = egui::pos2(proj.pos.x, proj.pos.y - 9.0);
    painter.text(
        text_pos,
        egui::Align2::CENTER_BOTTOM,
        label,
        egui::FontId::proportional(9.5),
        color,
    );
}

fn label_anchor(geometry: &GeoJsonGeometry) -> Option<GeoPoint> {
    match geometry {
        GeoJsonGeometry::Point(pt) => Some(*pt),
        GeoJsonGeometry::LineString(pts) if !pts.is_empty() => Some(pts[pts.len() / 2]),
        GeoJsonGeometry::MultiLineString(lines) if !lines.is_empty() && !lines[0].is_empty() => {
            Some(lines[0][lines[0].len() / 2])
        }
        GeoJsonGeometry::Polygon(rings) if !rings.is_empty() => {
            crate::model::ring_centroid(&rings[0])
        }
        GeoJsonGeometry::MultiPolygon(polys) if !polys.is_empty() && !polys[0].is_empty() => {
            crate::model::ring_centroid(&polys[0][0])
        }
        _ => None,
    }
}

pub(super) fn draw_global_topo(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    selected_root: Option<&std::path::Path>,
) {
    puffin::profile_function!();
    // Crossfade: full opacity at zoom ≤ 3.0, fade to zero by zoom 5.0.
    // SRTM globe tiles fade in from 1.5→3.0, so there is overlap in the
    // 3–5× range where both layers contribute before SRTM dominates.
    let alpha = (1.0 - (view.zoom - 3.0) / 2.0).clamp(0.0, 1.0);
    if alpha <= 0.01 {
        return;
    }

    let Some(topo) =
        contour_asset::load_global_topo(selected_root, view.zoom, painter.ctx().clone())
    else {
        return;
    };

    // Cap by contour count rather than by point count so the selection is
    // stable across frames.  Contours are already sorted by |elevation| in
    // render_globe_tiles, so this naturally keeps the lowest (most visible)
    // contours and drops the high-frequency minor ones when over budget.
    const MAX_GLOBE_TOPO_CONTOURS: usize = 2_000;
    let topo_slice = if topo.len() > MAX_GLOBE_TOPO_CONTOURS {
        &topo[..MAX_GLOBE_TOPO_CONTOURS]
    } else {
        topo.as_slice()
    };

    for contour in topo_slice {
        let major = (contour.elevation_m.round() as i32).rem_euclid(2_000) == 0;
        let color = if major {
            theme::hot_color()
        } else {
            theme::contour_color()
        };
        draw_geo_path(
            painter,
            layout,
            view,
            &contour.points,
            0.015,
            color.gamma_multiply(alpha),
            0.05 * alpha,
        );
    }
}

/// Draw SRTM focus-tile contours directly on the sphere surface.
/// Fades in from zoom 2.0 → 4.0, crossfading with the coarser global topo.
/// Because these go through `draw_geo_path` / `project_geo` they are
/// sphere-projected and rotate with the globe — no floating flat overlay.
pub(super) fn draw_srtm_on_globe(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    _lod: &GlobeLod,
    selected_root: Option<&std::path::Path>,
) {
    puffin::profile_function!();
    if view.zoom < 1.5 {
        return;
    }
    // Fade in over 1.5→3.0x.  Tiles are fixed-size (zoom_bucket=1, ~2.2°
    // half-extent) so they maintain constant apparent size on screen as the
    // globe grows rather than shrinking with each zoom step.
    let alpha = ((view.zoom - 1.5) / 1.5).clamp(0.0, 1.0);

    let Some(contours) = contour_asset::load_srtm_for_globe(
        selected_root,
        view.local_center,
        view.zoom,
        painter.ctx().clone(),
    ) else {
        return;
    };

    // Cap by contour count, not by point count, so the budget is stable across
    // frames.  render_globe_tiles sorts by |elevation|, so truncating here
    // keeps the lowest-elevation (most prominent) contours and drops minor
    // high-frequency ones — the right trade-off for globe-scale rendering.
    const MAX_GLOBE_SRTM_CONTOURS: usize = 3_000;
    let contour_slice = if contours.len() > MAX_GLOBE_SRTM_CONTOURS {
        &contours[..MAX_GLOBE_SRTM_CONTOURS]
    } else {
        contours.as_slice()
    };

    for contour in contour_slice {
        let major = (contour.elevation_m.round() as i32).rem_euclid(50) == 0;
        let color = if major {
            theme::hot_color()
        } else {
            theme::contour_color()
        };
        // Use the same altitude_scale as coastlines (0.022) so SRTM contours
        // sit on the sphere surface and don't parallax against the coastline layer.
        // lod.altitude_scale is designed for exaggerated local-terrain relief and
        // would push these contours visibly above the globe radius.
        draw_geo_path(
            painter,
            layout,
            view,
            &contour.points,
            0.020,
            color.gamma_multiply(alpha),
            0.08,
        );
    }
}

/// Draw lunar contour lines and feature labels on the globe when Moon Mode is active.
/// Contours are sourced from the SLDEM2015 JP2 tile cache; labels name major maria.
pub(super) fn draw_lunar_topo(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    selected_root: Option<&std::path::Path>,
) {
    puffin::profile_function!();
    // ── Contour lines ─────────────────────────────────────────────────────────
    // Fade in from zoom 0.6 → 1.4 so they don't clutter the full-disc view.
    let contour_alpha = ((view.zoom - 0.6) / 0.8).clamp(0.0, 1.0);
    if contour_alpha > 0.01 {
        if let Some(contours) = contour_asset::load_lunar_for_globe(
            selected_root,
            view.local_center,
            view.zoom,
            painter.ctx().clone(),
        ) {
            for contour in contours.iter() {
                // Major contours at multiples of 2× the minor interval.
                // At 500 m interval: every 1000 m is major.
                let major = (contour.elevation_m.round() as i32).rem_euclid(1_000) == 0;
                // Highland (positive) → warm grey; mare/basin (negative) → cool dark.
                let color = if contour.elevation_m >= 0.0 {
                    if major {
                        theme::hot_color()
                    } else {
                        theme::contour_color()
                    }
                } else {
                    // Below datum — bluish-grey to hint at the dark maria floors.
                    let base = egui::Color32::from_rgb(90, 100, 130);
                    if major {
                        base
                    } else {
                        base.gamma_multiply(0.55)
                    }
                };
                draw_geo_path(
                    painter,
                    layout,
                    view,
                    &contour.points,
                    0.015,
                    color.gamma_multiply(contour_alpha),
                    0.05 * contour_alpha,
                );
            }
        }
    }

    // ── Feature labels ────────────────────────────────────────────────────────
    if view.zoom < 0.8 {
        return;
    }
    let alpha = ((view.zoom - 0.8) / 0.8).clamp(0.0, 1.0);
    let label_color = theme::text_muted().gamma_multiply(alpha * 0.75);

    for &(name, lat, lon) in LUNAR_FEATURES {
        let Some(proj) = project_geo(layout, view, GeoPoint { lat, lon }, 0.0) else {
            continue;
        };
        if !proj.front_facing {
            continue;
        }
        painter.text(
            proj.pos,
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::monospace(10.0),
            label_color,
        );
    }
}

/// Draw Mars contour lines and feature labels on the globe when Mars Mode is active.
/// Contours are sourced from the CTX VRT; labels name major Martian features.
pub(super) fn draw_mars_topo(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    selected_root: Option<&std::path::Path>,
) {
    puffin::profile_function!();
    // ── Contour lines ─────────────────────────────────────────────────────────
    // Fade in from zoom 0.6 → 1.4 so they don't clutter the full-disc view.
    let contour_alpha = ((view.zoom - 0.6) / 0.8).clamp(0.0, 1.0);
    if contour_alpha > 0.01 {
        if let Some(contours) = contour_asset::load_mars_for_globe(
            selected_root,
            view.local_center,
            view.zoom,
            painter.ctx().clone(),
        ) {
            for contour in contours.iter() {
                let major = (contour.elevation_m.round() as i32).rem_euclid(1_000) == 0;
                let color = if contour.elevation_m >= 0.0 {
                    if major {
                        theme::hot_color()
                    } else {
                        theme::contour_color()
                    }
                } else {
                    // Below datum — rusty red to hint at the deep basins.
                    let base = egui::Color32::from_rgb(180, 80, 50);
                    if major {
                        base
                    } else {
                        base.gamma_multiply(0.55)
                    }
                };
                draw_geo_path(
                    painter,
                    layout,
                    view,
                    &contour.points,
                    0.015,
                    color.gamma_multiply(contour_alpha),
                    0.05 * contour_alpha,
                );
            }
        }
    }

    // ── Feature labels ────────────────────────────────────────────────────────
    if view.zoom < 0.8 {
        return;
    }
    let alpha = ((view.zoom - 0.8) / 0.8).clamp(0.0, 1.0);
    let label_color = theme::text_muted().gamma_multiply(alpha * 0.75);

    for &(name, lat, lon) in MARS_FEATURES {
        let Some(proj) = project_geo(layout, view, GeoPoint { lat, lon }, 0.0) else {
            continue;
        };
        if !proj.front_facing {
            continue;
        }
        painter.text(
            proj.pos,
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::monospace(10.0),
            label_color,
        );
    }
}
