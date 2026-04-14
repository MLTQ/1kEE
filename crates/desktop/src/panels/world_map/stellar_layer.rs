/// Stellar Correspondence Layer — time-varying rendering.
///
/// Maps each star/planet from the celestial sphere onto its geographic position
/// (GP) on Earth:
///
///   lat  = Declination
///   lon  = RA − GMST(t)          (normalized to −180…180°)
///
/// With `stellar_precess` enabled, star coordinates are precessed from J2000.0
/// to the current epoch before the GMST subtraction, which is essential for
/// epochs more than a century from J2000 — e.g. the pole star shifts visibly
/// over centuries, and at Göbekli Tepe era Thuban was the pole star, not Polaris.

use crate::model::{GeoPoint, GlobeViewState};
use crate::planet_ephemeris::{self, Planet};
use crate::stellar_catalog;
use crate::stellar_time;

use super::globe_scene::{project_geo, GlobeLayout};

// ── Stars ─────────────────────────────────────────────────────────────────────

pub(super) fn draw_stellar_correspondence(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    stellar_jd: f64,
    stellar_precess: bool,
) {
    puffin::profile_function!();
    let gmst = stellar_time::gmst_deg(stellar_jd);

    for star in stellar_catalog::STARS {
        let (ra, dec) = if stellar_precess {
            stellar_time::precess_j2000(star.ra_deg as f64, star.dec_deg as f64, stellar_jd)
        } else {
            (star.ra_deg as f64, star.dec_deg as f64)
        };

        let lon_raw = (ra - gmst).rem_euclid(360.0);
        let lon = if lon_raw > 180.0 { lon_raw - 360.0 } else { lon_raw };

        let location = GeoPoint { lat: dec as f32, lon: lon as f32 };
        let Some(proj) = project_geo(layout, view, location, 0.0) else { continue };
        if !proj.front_facing { continue; }

        let radius = ((3.0 - star.mag) * 0.55).clamp(0.9, 4.5);

        if star.mag < 1.0 {
            let bloom_alpha = (((1.0 - star.mag) / 2.0) * 30.0).clamp(10.0, 30.0) as u8;
            painter.circle_filled(
                proj.pos,
                radius * 2.8,
                egui::Color32::from_rgba_unmultiplied(180, 210, 255, bloom_alpha),
            );
        }

        let core_alpha = (((3.5 - star.mag) / 5.5) * 220.0).clamp(80.0, 220.0) as u8;
        painter.circle_filled(
            proj.pos,
            radius,
            egui::Color32::from_rgba_unmultiplied(215, 228, 255, core_alpha),
        );

        if star.mag < 2.5 {
            painter.circle_filled(
                proj.pos,
                (radius * 0.35).max(0.6),
                egui::Color32::from_rgba_unmultiplied(240, 246, 255, 240),
            );
        }

        if !star.name.is_empty() && star.mag < 2.2 {
            painter.text(
                proj.pos + egui::vec2(radius + 3.0, 0.0),
                egui::Align2::LEFT_CENTER,
                star.name,
                egui::FontId::monospace(8.5),
                egui::Color32::from_rgba_unmultiplied(185, 208, 248, 155),
            );
        }
    }
}

// ── Planets ───────────────────────────────────────────────────────────────────

pub(super) fn draw_planets(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    stellar_jd: f64,
) {
    puffin::profile_function!();
    let gmst = stellar_time::gmst_deg(stellar_jd);

    for &planet in planet_ephemeris::ALL_PLANETS {
        let Some((ra, dec)) = planet_ephemeris::geocentric_radec(planet, stellar_jd) else {
            continue;
        };
        let lon_raw = (ra - gmst).rem_euclid(360.0);
        let lon = if lon_raw > 180.0 { lon_raw - 360.0 } else { lon_raw };

        let location = GeoPoint { lat: dec as f32, lon: lon as f32 };
        let Some(proj) = project_geo(layout, view, location, 0.0) else { continue };
        if !proj.front_facing { continue; }

        let col = planet.color();
        let r   = planet.radius();

        // Outer glow halo
        painter.circle_filled(proj.pos, r * 2.0, col.gamma_multiply(0.10));
        painter.circle_filled(proj.pos, r * 1.3, col.gamma_multiply(0.25));

        // Core body
        painter.circle_filled(proj.pos, r, col);
        // Bright centre
        painter.circle_filled(proj.pos, r * 0.35, col.gamma_multiply(1.6).linear_multiply(1.5));

        // Label
        painter.text(
            proj.pos + egui::vec2(r + 4.0, 0.0),
            egui::Align2::LEFT_CENTER,
            planet.label(),
            egui::FontId::monospace(9.5),
            col.gamma_multiply(0.88),
        );
    }
}

// ── Planet trails ─────────────────────────────────────────────────────────────

pub(super) fn draw_planet_trails(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    stellar_jd: f64,
    trail_years: f32,
) {
    puffin::profile_function!();
    const TRAIL_POINTS: usize = 240;

    for &planet in planet_ephemeris::ALL_PLANETS {
        let span_days = if trail_years > 0.0 {
            trail_years as f64 * 365.25
        } else {
            planet.default_trail_days()
        };

        let trail = planet_ephemeris::planet_trail(planet, stellar_jd, span_days, TRAIL_POINTS);
        if trail.len() < 2 {
            continue;
        }

        let col = planet.color();
        draw_trail_path(painter, layout, view, &trail, col);
    }
}

/// Draw a geographic path (lon, lat pairs), breaking at the antimeridian and
/// at the horizon (back-facing segments).
fn draw_trail_path(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    trail: &[(f32, f32)],
    col: egui::Color32,
) {
    let n = trail.len();
    let mut segment: Vec<egui::Pos2> = Vec::with_capacity(16);

    for idx in 0..n {
        let (lon, lat) = trail[idx];
        let location = GeoPoint { lat, lon };

        // Detect antimeridian jump: if consecutive lons differ by > 180°, break
        let antimeridian_jump = if idx > 0 {
            (trail[idx].0 - trail[idx - 1].0).abs() > 180.0
        } else {
            false
        };

        if antimeridian_jump {
            flush_segment(painter, &mut segment, col);
        }

        match project_geo(layout, view, location, 0.0) {
            Some(p) if p.front_facing => {
                // Fade from old (tail) to new (head): oldest = 0, newest = n-1
                let t = idx as f32 / (n - 1) as f32;
                let alpha = (t * 180.0).clamp(15.0, 180.0) as u8;
                let _ = alpha; // alpha baked into stroke below
                segment.push(p.pos);
            }
            _ => {
                flush_segment(painter, &mut segment, col);
            }
        }
    }
    flush_segment(painter, &mut segment, col);
}

fn flush_segment(painter: &egui::Painter, segment: &mut Vec<egui::Pos2>, col: egui::Color32) {
    if segment.len() >= 2 {
        painter.add(egui::Shape::line(
            std::mem::take(segment),
            egui::Stroke::new(1.2, col.gamma_multiply(0.55)),
        ));
    } else {
        segment.clear();
    }
}
