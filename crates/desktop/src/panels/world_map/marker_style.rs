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
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 || alpha <= 0.001 || scale <= 0.0 {
        return;
    }
    // Unit perpendicular vector in screen space for ribbon expansion.
    let nx = -dy / len;
    let ny = dx / len;

    // Emit all 3 atmospheric halo ribbons + 2 core ribbons in ONE batched egui::Mesh
    // instead of 49 separate painter.line_segment shapes (smoother gradient + ~10x faster).
    const HALO_STEPS: usize = 7;
    const CORE_STEPS: usize = 10;
    let mut mesh = egui::Mesh::default();
    mesh.vertices.reserve((3 * (HALO_STEPS + 1) + 2 * (CORE_STEPS + 1)) * 2);
    mesh.indices.reserve((3 * HALO_STEPS + 2 * CORE_STEPS) * 6);

    let push_ribbon = |mesh: &mut egui::Mesh,
                       steps: usize,
                       width_fn: &dyn Fn(f32) -> f32,
                       alpha_fn: &dyn Fn(f32) -> f32| {
        let start = mesh.vertices.len() as u32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let cx = base.x + dx * t;
            let cy = base.y + dy * t;
            let hw = width_fn(t) * 0.5;
            let c = col.gamma_multiply(alpha_fn(t));
            mesh.colored_vertex(egui::pos2(cx - nx * hw, cy - ny * hw), c);
            mesh.colored_vertex(egui::pos2(cx + nx * hw, cy + ny * hw), c);
        }
        for i in 0..steps as u32 {
            let v = start + i * 2;
            mesh.add_triangle(v, v + 1, v + 2);
            mesh.add_triangle(v + 1, v + 3, v + 2);
        }
    };

    // Three wide-to-narrow atmospheric halo ribbons.
    for (w_mult, a_mult) in [(22.0_f32, 0.04_f32), (11.0, 0.08), (4.5, 0.16)] {
        push_ribbon(
            &mut mesh,
            HALO_STEPS,
            &|t| {
                let a = (1.0 - t).powi(2) * alpha;
                (w_mult * a).max(0.5) * scale
            },
            &|t| (1.0 - t).powi(2) * alpha * a_mult,
        );
    }

    // Tapering glow + bright core ribbons.
    for (w_mult, w_min, a_mult) in [(4.0_f32, 0.4_f32, 0.30_f32), (1.7, 0.3, 0.96)] {
        push_ribbon(
            &mut mesh,
            CORE_STEPS,
            &|t| {
                let falloff = 1.0 - t;
                (w_mult * falloff.powf(0.7)).max(w_min) * scale
            },
            &|t| (1.0 - t).powi(3) * alpha * a_mult,
        );
    }

    painter.add(egui::Shape::mesh(mesh));
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
