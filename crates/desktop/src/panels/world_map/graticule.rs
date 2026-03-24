use crate::model::{GeoPoint, GlobeViewState};
use crate::theme;

use super::globe_scene::{GlobeLayout, project_geo};

// ── CPU graticule ─────────────────────────────────────────────────────────────

/// Draw the lat/lon grid as crisp CPU polylines on top of all terrain layers.
///
/// Grid step adapts to zoom (matches GPU shader thresholds):
///   radius < 120 px → 30°,  < 300 px → 15°,  ≥ 300 px → 10°
///
/// Special lines (equator, tropics ±23.44°, polar circles ±66.56°, prime
/// meridian) use `hot_color()`; major 30° lines use brighter `wireframe_color()`;
/// minor subdivisions use plain `wireframe_color()`.
pub(super) fn draw_graticule(painter: &egui::Painter, layout: &GlobeLayout, view: &GlobeViewState) {
    let step: f32 = if layout.radius >= 300.0 { 10.0 }
                    else if layout.radius >= 120.0 { 15.0 }
                    else { 30.0 };

    let hot   = theme::hot_color();
    let major = theme::wireframe_color().gamma_multiply(1.8);
    let minor = theme::wireframe_color();

    const SPECIAL_LATS: &[f32] = &[0.0, 23.44, -23.44, 66.56, -66.56];

    // ── Latitude parallels ─────────────────────────────────────────────────
    // Iterate at `step` intervals, then inject special latitudes that aren't
    // already represented within 0.5° of a grid line.
    let mut lats: Vec<(f32, bool, bool)> = Vec::new(); // (lat, is_special, is_major_30)
    let mut l = -90.0_f32 + step;
    while l < 90.0 - step * 0.1 {
        let is_sp = SPECIAL_LATS.iter().any(|&s| (s - l).abs() < 0.5);
        let is_m  = (l / 30.0).fract().abs().min(1.0 - (l / 30.0).fract().abs()) < 0.02;
        lats.push((l, is_sp, is_m));
        l += step;
    }
    for &sl in SPECIAL_LATS {
        if sl.abs() < 89.5 && !lats.iter().any(|&(ll, _, _)| (ll - sl).abs() < 0.5) {
            lats.push((sl, true, false));
        }
    }
    for (lat, is_sp, is_m) in lats {
        let (col, w) = if is_sp        { (hot.gamma_multiply(0.72),   1.15) }
                       else if is_m    { (major.gamma_multiply(0.55),  0.95) }
                       else            { (minor.gamma_multiply(0.28),  0.80) };
        // One point every 5° of longitude for smooth circles near the equator.
        let pts: Vec<GeoPoint> = (-36..=36)
            .map(|i| GeoPoint { lat, lon: i as f32 * 5.0 })
            .collect();
        draw_graticule_path(painter, layout, view, &pts, col, w);
    }

    // ── Longitude meridians ────────────────────────────────────────────────
    let mut lon = -180.0_f32 + step;
    while lon <= 180.0 - step * 0.1 {
        let is_pm = lon.abs() < 0.5;
        let is_m  = (lon / 30.0).fract().abs().min(1.0 - (lon / 30.0).fract().abs()) < 0.02;
        let (col, w) = if is_pm       { (hot.gamma_multiply(0.72),   1.15) }
                       else if is_m   { (major.gamma_multiply(0.55),  0.95) }
                       else           { (minor.gamma_multiply(0.28),  0.80) };
        // One point every 3° of latitude for smooth meridian curves.
        let pts: Vec<GeoPoint> = (-30..=30)
            .map(|i| GeoPoint { lat: i as f32 * 3.0, lon })
            .collect();
        draw_graticule_path(painter, layout, view, &pts, col, w);
        lon += step;
    }
    // Prime meridian is handled by the `is_pm` check inside the loop above.
}

/// Variant of `draw_geo_path` with explicit front/back stroke widths,
/// used by the graticule so every line can have its own weight.
fn draw_graticule_path(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    path: &[GeoPoint],
    color: egui::Color32,
    width: f32,
) {
    const ALT: f32 = 0.022;         // sits on sphere surface, same as coastlines
    const BACK_ALPHA: f32 = 0.14;   // faint back-hemisphere ghost reinforces 3D feel

    let mut front_seg: Vec<egui::Pos2> = Vec::new();
    let mut back_seg:  Vec<egui::Pos2> = Vec::new();

    for &point in path {
        match project_geo(layout, view, point, ALT) {
            Some(proj) if proj.front_facing => {
                if back_seg.len() >= 2 {
                    painter.add(egui::Shape::line(
                        std::mem::take(&mut back_seg),
                        egui::Stroke::new(width * 0.5, color.gamma_multiply(BACK_ALPHA)),
                    ));
                }
                back_seg.clear();
                front_seg.push(proj.pos);
            }
            Some(proj) => {
                if front_seg.len() >= 2 {
                    painter.add(egui::Shape::line(
                        std::mem::take(&mut front_seg),
                        egui::Stroke::new(width, color),
                    ));
                }
                front_seg.clear();
                back_seg.push(proj.pos);
            }
            None => {
                if front_seg.len() >= 2 {
                    painter.add(egui::Shape::line(
                        std::mem::take(&mut front_seg),
                        egui::Stroke::new(width, color),
                    ));
                }
                if back_seg.len() >= 2 {
                    painter.add(egui::Shape::line(
                        std::mem::take(&mut back_seg),
                        egui::Stroke::new(width * 0.5, color.gamma_multiply(BACK_ALPHA)),
                    ));
                }
                front_seg.clear();
                back_seg.clear();
            }
        }
    }
    if front_seg.len() >= 2 {
        painter.add(egui::Shape::line(
            front_seg,
            egui::Stroke::new(width, color),
        ));
    }
    if back_seg.len() >= 2 {
        painter.add(egui::Shape::line(
            back_seg,
            egui::Stroke::new(width * 0.5, color.gamma_multiply(BACK_ALPHA)),
        ));
    }
}
