//! Intersect the actual display triangles at selection time. The section then
//! reads the finer numeric DEM, so holes and original metre elevations survive.
use super::{Terrain, camera::Camera};
use egui::{Color32, Pos2, Rect, Stroke};
use glam::Vec3;
use lidar_pipeline::region::Region;

#[derive(Default)]
pub(crate) struct Profile {
    pub enabled: bool,
    start: Option<Vec3>,
    end: Option<Vec3>,
    samples: Vec<Option<Vec3>>,
    distance: f32,
    range: Option<[f32; 2]>,
    missed: bool,
}
const INK: Color32 = Color32::from_rgb(231, 159, 82);

fn pick(dem: &Terrain, camera: &Camera, rect: Rect, p: Pos2) -> Option<Vec3> {
    if !rect.contains(p) {
        return None;
    }
    let (right, up, forward) = camera.basis();
    let offset = (p - rect.center()) * (2. * 30f32.to_radians().tan() / rect.height());
    let dir = Vec3::from(forward) + Vec3::from(right) * offset.x - Vec3::from(up) * offset.y;
    let eye = Vec3::from(camera.eye);
    let mut closest = f32::INFINITY;
    for tri in dem.indices.as_chunks::<3>().0 {
        let vertex = |i: usize| Vec3::from(dem.batch.points[tri[i] as usize].position);
        let a = vertex(0);
        let ab = vertex(1) - a;
        let ac = vertex(2) - a;
        let cross = dir.cross(ac);
        let det = ab.dot(cross);
        if det.abs() < 1e-8 {
            continue;
        }
        let delta = eye - a;
        let u = delta.dot(cross) / det;
        if !(0. ..=1.).contains(&u) {
            continue;
        }
        let cross = delta.cross(ab);
        let v = dir.dot(cross) / det;
        if v < 0. || u + v > 1. {
            continue;
        }
        let t = ac.dot(cross) / det;
        if t >= 0.1 && t < closest && (eye + t * dir).truncate().length() <= camera.radius_m {
            closest = t;
        }
    }
    closest.is_finite().then(|| eye + closest * dir)
}
impl Profile {
    fn select(&mut self, point: Option<Vec3>, dem: &Terrain, region: Region) {
        let Some(p) = point else {
            self.missed = true;
            return;
        };
        self.missed = false;
        if let Some(a) = self.start
            && self.end.is_none()
        {
            self.end = Some(p);
            self.distance = (p - a).truncate().length();
            let dx = dem.scale[0].abs() * 111_320. * region.lat.to_radians().cos();
            let dy = dem.scale[1].abs() * 111_320.;
            let intervals = (2. * f64::from(self.distance) / dx.min(dy))
                .ceil()
                .clamp(1., 8192.) as usize;
            self.range = None;
            self.samples = (0..=intervals)
                .map(|i| {
                    let xy = a.lerp(p, i as f32 / intervals as f32);
                    let [lon, lat] = region.location(xy.x.into(), xy.y.into());
                    let h = dem.sample(lon, lat)?;
                    let range = self.range.get_or_insert([h, h]);
                    range[0] = range[0].min(h);
                    range[1] = range[1].max(h);
                    Some(Vec3::new(xy.x, xy.y, h))
                })
                .collect();
        } else {
            self.start = Some(p);
            self.end = None;
            self.samples.clear();
            self.range = None;
        }
    }
    pub fn input(
        &mut self,
        response: &egui::Response,
        dem: &Terrain,
        region: Region,
        camera: &Camera,
    ) {
        let rect = response.rect;
        if response.drag_started_by(egui::PointerButton::Primary) {
            self.start = None;
            self.end = None;
            self.samples.clear();
            self.range = None;
            let start = response
                .ctx
                .input(|i| i.pointer.press_origin())
                .and_then(|p| pick(dem, camera, rect, p));
            self.select(start, dem, region);
        }
        if (response.clicked_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Primary))
            && let Some(p) = response.interact_pointer_pos()
        {
            self.select(pick(dem, camera, rect, p), dem, region);
        }
    }
    #[cfg(test)]
    pub fn measured_range(&self) -> Option<[f32; 2]> {
        self.range
    }
    pub fn chart(&self, ui: &mut egui::Ui) {
        if self.missed {
            ui.small("Choose a point on the measured terrain.");
        }
        if self.end.is_none() {
            ui.small(if self.start.is_some() {
                "Click the end of the section."
            } else {
                "Drag across the terrain or click two endpoints."
            });
            return;
        }
        let Some([lo, hi]) = self.range else {
            ui.label("No measured elevations along this section.");
            return;
        };
        ui.label(format!(
            "A to B   {:.0} m horizontal    ·    {:.2} to {:.2} m elevation    ·    {:.2} m relief",
            self.distance,
            lo,
            hi,
            hi - lo
        ));
        let (response, painter) =
            ui.allocate_painter(egui::vec2(ui.available_width(), 92.), egui::Sense::hover());
        let margin = (response.rect.width() * 0.2).min(64.);
        let area = Rect::from_min_max(
            response.rect.min + egui::vec2(margin, 8.),
            response.rect.max - egui::vec2(8., 19.),
        );
        painter.rect_filled(response.rect, 3., Color32::from_rgb(21, 32, 39));
        let span = (hi - lo).max(0.1);
        let project = |i: usize, h: f32| {
            Pos2::new(
                area.left() + area.width() * i as f32 / (self.samples.len() - 1) as f32,
                area.center().y - area.height() * (h - (lo + hi) * 0.5) / span,
            )
        };
        let grid = Stroke::new(0.5_f32, Color32::from_gray(66));
        let label = Color32::from_gray(184);
        for h in if hi == lo { vec![lo] } else { vec![lo, hi] } {
            let y = project(0, h).y;
            painter.hline(area.x_range(), y, grid);
            painter.text(
                Pos2::new(area.left() - 5., y),
                egui::Align2::RIGHT_CENTER,
                format!("{h:.1} m"),
                egui::FontId::monospace(10.),
                label,
            );
        }
        for (x, align, dist) in [
            (area.left(), egui::Align2::LEFT_TOP, 0.),
            (area.right(), egui::Align2::RIGHT_TOP, self.distance),
        ] {
            painter.text(
                Pos2::new(x, area.bottom() + 4.),
                align,
                format!("{dist:.0} m"),
                egui::FontId::monospace(10.),
                label,
            );
        }
        draw_runs(
            &painter,
            self.samples
                .iter()
                .enumerate()
                .map(|(i, p)| p.map(|p| project(i, p.z))),
        );
        if let Some(cursor) = response.hover_pos() {
            let t = ((cursor.x - area.left()) / area.width().max(1.)).clamp(0., 1.);
            let index = (t * (self.samples.len() - 1) as f32).round() as usize;
            let x = project(index, lo).x;
            painter.vline(x, area.y_range(), grid);
            if let Some(sample) = self.samples[index] {
                painter.circle_filled(project(index, sample.z), 2.5, INK);
            }
            let z = self.samples[index]
                .map_or_else(|| "no data".into(), |p| format!("{:.2} m elevation", p.z));
            response.on_hover_text(format!("{:.0} m from A · {z}", t * self.distance));
        }
    }
    pub fn draw_path(&self, painter: &egui::Painter, rect: Rect, camera: &Camera) {
        draw_runs(
            painter,
            self.samples
                .iter()
                .map(|p| p.and_then(|p| camera.project(p.into(), rect))),
        );
        for (label, point) in [("A", self.start), ("B", self.end)] {
            if let Some(pos) = point.and_then(|p| camera.project(p.into(), rect)) {
                painter.circle_filled(pos, 3., INK);
                painter.text(
                    pos + egui::vec2(5., 0.),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::monospace(12.),
                    Color32::WHITE,
                );
            }
        }
    }
}
fn draw_runs(painter: &egui::Painter, points: impl Iterator<Item = Option<Pos2>>) {
    let mut line = Vec::new();
    for p in points.chain(std::iter::once(None)) {
        if let Some(p) = p {
            line.push(p);
        } else if line.len() >= 2 {
            painter.add(egui::Shape::line(
                std::mem::take(&mut line),
                Stroke::new(1.5_f32, INK),
            ));
        } else if let Some(p) = line.pop() {
            painter.circle_filled(p, 1.5, INK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target_mode::{Batch, Vertex};
    use std::sync::Arc;
    #[test]
    fn picking_hits_visible_triangle_and_profile_preserves_nodata() {
        let region = Region {
            lon: 0.,
            lat: 0.,
            radius_m: 1000.,
        };
        let points = [[-500., -500., 0.], [500., -500., 0.], [0., 500., 0.]]
            .map(|position| Vertex {
                position,
                color: [255; 4],
                class: 0,
            })
            .to_vec();
        let mut dem = Terrain {
            batch: Arc::new(Batch {
                points,
                id: 1,
                source: None,
            }),
            indices: vec![0, 1, 2],
            label: String::new(),
            height_range: crate::target_mode::heatmap::HeightRange { low: 0., high: 0. },
            navd88: true,
            width: 9,
            height: 9,
            origin: [-0.0045, 0.0045],
            scale: [0.001, -0.001],
            heights: vec![0.; 81],
        };
        for row in dem.heights.chunks_mut(9) {
            row[4] = f32::NAN;
            row[6] = 3.;
            row[7] = 3.;
        }
        let mut camera = Camera::for_radius(1000.);
        camera.reset(0.);
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800., 600.));
        let pos = camera.project([25., 60., 0.], rect).unwrap();
        let p = pick(&dem, &camera, rect, pos).unwrap();
        assert!((p - Vec3::new(25., 60., 0.)).length() < 0.05);
        assert!(pick(&dem, &camera, rect, rect.left_top()).is_none());
        let mut profile = Profile::default();
        profile.select(Some(Vec3::new(-300., 0., 0.)), &dem, region);
        profile.select(Some(Vec3::new(300., 0., 0.)), &dem, region);
        assert_eq!(profile.distance, 600.);
        assert!(
            profile.samples.first().unwrap().is_some() && profile.samples.last().unwrap().is_some()
        );
        assert!(profile.samples.iter().any(Option::is_none));
        let [lo, hi] = profile.range.unwrap();
        assert_eq!(lo, 0.);
        assert!((hi - 3.).abs() < 1e-5);
    }
}
