//! Shared camera/event light style and the geometry used by hover and clicks.
#[derive(Clone, Debug)]
pub(super) struct MapMarker {
    pub id: String,
    pub base: egui::Pos2,
    pub tip: egui::Pos2,
    pub scale: f32,
}

impl MapMarker {
    pub fn new(id: &str, base: egui::Pos2, tip: egui::Pos2, scale: f32) -> Self {
        Self {
            id: id.to_owned(),
            base,
            tip,
            scale,
        }
    }

    fn hit_distance(&self, pointer: egui::Pos2) -> Option<f32> {
        let base_distance = pointer.distance(self.base);
        // Keep small markers usable, while large markers acquire a matching target.
        if base_distance <= (10.0 * self.scale).max(4.0) {
            return Some(base_distance);
        }
        // Only the visibly lit lower portion is interactive; the transparent tip
        // must not steal clicks from another marker or map navigation.
        let beam = (self.tip - self.base) * 0.65;
        let length_sq = beam.length_sq();
        if length_sq < 1.0 {
            return None;
        }
        let t = ((pointer - self.base).dot(beam) / length_sq).clamp(0.0, 1.0);
        let distance = pointer.distance(self.base + beam * t);
        (distance <= (4.0 * self.scale).max(2.0)).then_some(distance + 10.0 * self.scale)
    }
}

pub(super) fn pick(markers: &[MapMarker], pointer: egui::Pos2) -> Option<&MapMarker> {
    markers
        .iter()
        .filter_map(|marker| marker.hit_distance(pointer).map(|d| (marker, d)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(marker, _)| marker)
}

/// Local projection keeps the terrain vertical axis screen-up at every yaw.
/// Scene code supplies the ground point from the visible terrain surface.
pub(super) fn local_tip(base: egui::Pos2, height: f32, scale: f32) -> egui::Pos2 {
    base - egui::vec2(0.0, height * scale)
}

pub(super) fn draw_beam(
    painter: &egui::Painter,
    base: egui::Pos2,
    tip: egui::Pos2,
    col: egui::Color32,
    alpha: f32,
    scale: f32,
) {
    let dx = tip.x - base.x;
    let dy = tip.y - base.y;

    // Atmospheric halos — taper in both width and alpha toward the tip.
    const HALO_SEGS: u32 = 7;
    for i in 0..HALO_SEGS {
        let t0 = i as f32 / HALO_SEGS as f32;
        let t1 = (i + 1) as f32 / HALO_SEGS as f32;
        let tm = (t0 + t1) * 0.5;
        let a = (1.0 - tm).powi(2) * alpha;
        let p0 = egui::pos2(base.x + dx * t0, base.y + dy * t0);
        let p1 = egui::pos2(base.x + dx * t1, base.y + dy * t1);
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new((22.0 * a).max(0.5) * scale, col.gamma_multiply(0.04 * a)),
        );
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new((11.0 * a).max(0.5) * scale, col.gamma_multiply(0.08 * a)),
        );
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new((4.5 * a).max(0.5) * scale, col.gamma_multiply(0.16 * a)),
        );
    }

    // Tapering core — cubic alpha, narrows to a spike.
    const SEGS: u32 = 14;
    for i in 0..SEGS {
        let t0 = i as f32 / SEGS as f32;
        let t1 = (i + 1) as f32 / SEGS as f32;
        let tm = (t0 + t1) * 0.5;
        let falloff = 1.0 - tm;
        let a = falloff.powi(3) * alpha;
        let w_glow = (4.0 * falloff.powf(0.7)).max(0.4);
        let w_core = (1.7 * falloff.powf(0.7)).max(0.3);
        let p0 = egui::pos2(base.x + dx * t0, base.y + dy * t0);
        let p1 = egui::pos2(base.x + dx * t1, base.y + dy * t1);
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new(w_glow * scale, col.gamma_multiply(a * 0.30)),
        );
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new(w_core * scale, col.gamma_multiply(a * 0.96)),
        );
    }
}

pub(super) fn draw_camera_spire(
    painter: &egui::Painter,
    base: egui::Pos2,
    tip: egui::Pos2,
    is_selected: bool,
    scale: f32,
) {
    let color = if is_selected {
        crate::theme::marker_camera_ring()
    } else {
        crate::theme::camera_color()
    };
    draw_beam(painter, base, tip, color, 1.0, scale);
    painter.circle_stroke(
        base,
        4.8 * scale,
        egui::Stroke::new(3.5 * scale, color.gamma_multiply(0.10)),
    );
    painter.circle_stroke(
        base,
        3.8 * scale,
        egui::Stroke::new(1.0 * scale, color.gamma_multiply(0.60)),
    );
    painter.circle_filled(base, 2.2 * scale, color);
    if is_selected {
        painter.circle_stroke(base, 8.0 * scale, egui::Stroke::new(1.1 * scale, color));
    }
}

#[cfg(test)]
#[path = "marker_style_tests.rs"]
mod tests;
