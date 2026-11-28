//! Smart, upright, collision-free rotated index-contour elevation labels for Local view.

use super::contour_asset::ContourPath;
use crate::model::GeoPoint;
use crate::theme;
use egui::{Color32, FontId, Pos2, Rect, Vec2};
use std::collections::HashSet;

const CELL_PX: f32 = 58.0;
const MAX_LABELS_PER_FRAME: usize = 64;
const MIN_SEGMENT_PX: f32 = 22.0;

/// Draw rotated elevation labels along index contours (`elevation_m % major_interval == 0`)
/// using low-curvature segment selection, upright angle normalization, a grid occupancy
/// collision map, and terrain-tinted knockout halos.
pub(crate) fn draw(
    painter: &egui::Painter,
    viewport: Rect,
    contours: &[ContourPath],
    major_interval_m: i32,
    project: impl Fn(GeoPoint, f32) -> Option<Pos2>,
) {
    if contours.is_empty() || major_interval_m <= 0 {
        return;
    }
    let bounds = viewport.shrink(28.0);
    if bounds.width() < 80.0 || bounds.height() < 80.0 {
        return;
    }

    let font = FontId::proportional(10.0);
    let text_col = if theme::is_atlas() {
        Color32::from_rgb(76, 62, 46)
    } else if theme::is_abyss() {
        Color32::from_rgb(185, 255, 248)
    } else {
        theme::hot_color().lerp_to_gamma(Color32::WHITE, 0.35)
    };
    let halo_col = theme::canvas_background().gamma_multiply(0.84);

    let mut occupied: HashSet<(i32, i32)> = HashSet::with_capacity(MAX_LABELS_PER_FRAME * 2);
    let mut drawn = 0usize;
    let clipped = painter.with_clip_rect(viewport);

    for path in contours {
        if drawn >= MAX_LABELS_PER_FRAME {
            break;
        }
        let elev_i = path.elevation_m.round() as i32;
        if elev_i == 0 || elev_i.rem_euclid(major_interval_m) != 0 {
            continue;
        }
        if path.points.len() < 3 {
            continue;
        }
        let Some((center, angle)) = select_label_anchor(path, bounds, &project) else {
            continue;
        };

        let cell = (
            (center.x / CELL_PX).floor() as i32,
            (center.y / CELL_PX).floor() as i32,
        );
        // Reject if this cell or an immediate neighbor already holds a label.
        if (-1..=1).any(|dy| (-1..=1).any(|dx| occupied.contains(&(cell.0 + dx, cell.1 + dy)))) {
            continue;
        }
        occupied.insert(cell);

        let label = format_elevation(elev_i);
        let galley = clipped.layout_no_wrap(label, font.clone(), text_col);
        let half = galley.size() * 0.5;
        let (sin_a, cos_a) = angle.sin_cos();

        // Draw a rotated knockout pill directly underneath the text so the contour breaks cleanly.
        let pad = Vec2::new(3.5, 1.5);
        let hw = half.x + pad.x;
        let hh = half.y + pad.y;
        let corners = [
            rotate_offset(center, Vec2::new(-hw, -hh), cos_a, sin_a),
            rotate_offset(center, Vec2::new(hw, -hh), cos_a, sin_a),
            rotate_offset(center, Vec2::new(hw, hh), cos_a, sin_a),
            rotate_offset(center, Vec2::new(-hw, hh), cos_a, sin_a),
        ];
        clipped.add(egui::Shape::convex_polygon(
            corners.to_vec(),
            halo_col,
            egui::Stroke::NONE,
        ));

        // TextShape rotates around `pos` (top-left of galley), so back-offset by rotated half-size.
        let origin = rotate_offset(center, -half, cos_a, sin_a);
        let mut shape = egui::epaint::TextShape::new(origin, galley, text_col);
        shape.angle = angle;
        clipped.add(egui::Shape::Text(shape));
        drawn += 1;
    }
}

/// Choose the straightest, longest visible interior segment along a contour path.
fn select_label_anchor(
    path: &ContourPath,
    bounds: Rect,
    project: &impl Fn(GeoPoint, f32) -> Option<Pos2>,
) -> Option<(Pos2, f32)> {
    let n = path.points.len();
    let step = (n / 18).max(1);
    let mut best: Option<(f32, Pos2, f32)> = None;

    let mut i = step;
    while i + step < n {
        let p_prev = project(path.points[i - step], path.elevation_m);
        let p_curr = project(path.points[i], path.elevation_m);
        let p_next = project(path.points[i + step], path.elevation_m);
        i += step;

        let (Some(a), Some(b), Some(c)) = (p_prev, p_curr, p_next) else {
            continue;
        };
        if !bounds.contains(b) {
            continue;
        }
        let v1 = b - a;
        let v2 = c - b;
        let l1 = v1.length();
        let l2 = v2.length();
        if l1 + l2 < MIN_SEGMENT_PX {
            continue;
        }
        let straightness = (v1.normalized()).dot(v2.normalized());
        if straightness < 0.94 {
            // Skip sharp hairpins (> ~20° turn).
            continue;
        }
        let chord = c - a;
        let chord_len = chord.length();
        if chord_len < MIN_SEGMENT_PX {
            continue;
        }
        let score = straightness * chord_len.min(120.0);
        if best.is_none_or(|(best_score, _, _)| score > best_score) {
            let raw_angle = chord.y.atan2(chord.x);
            let upright = normalize_upright_angle(raw_angle);
            best = Some((score, b, upright));
        }
    }
    best.map(|(_, pos, angle)| (pos, angle))
}

#[inline]
fn rotate_offset(center: Pos2, offset: Vec2, cos_a: f32, sin_a: f32) -> Pos2 {
    Pos2::new(
        center.x + offset.x * cos_a - offset.y * sin_a,
        center.y + offset.x * sin_a + offset.y * cos_a,
    )
}

/// Keep rotated text strictly upright (`[-PI/2, PI/2]`) so labels never render upside down.
pub(crate) fn normalize_upright_angle(angle: f32) -> f32 {
    let mut a = angle;
    while a > std::f32::consts::FRAC_PI_2 {
        a -= std::f32::consts::PI;
    }
    while a < -std::f32::consts::FRAC_PI_2 {
        a += std::f32::consts::PI;
    }
    a
}

pub(crate) fn format_elevation(elev_m: i32) -> String {
    let neg = elev_m < 0;
    let abs = elev_m.unsigned_abs();
    if abs >= 1000 {
        format!(
            "{}{},{:03} m",
            if neg { "-" } else { "" },
            abs / 1000,
            abs % 1000
        )
    } else {
        format!("{elev_m} m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upright_angle_always_stays_readable() {
        for deg in (-360..=360).step_by(15) {
            let rad = (deg as f32).to_radians();
            let norm = normalize_upright_angle(rad);
            assert!(
                norm >= -std::f32::consts::FRAC_PI_2 - 1e-4
                    && norm <= std::f32::consts::FRAC_PI_2 + 1e-4,
                "deg={deg} norm={norm}"
            );
        }
    }

    #[test]
    fn elevation_formatting_adds_thousands_separator() {
        assert_eq!(format_elevation(50), "50 m");
        assert_eq!(format_elevation(1250), "1,250 m");
        assert_eq!(format_elevation(-2400), "-2,400 m");
    }
}
