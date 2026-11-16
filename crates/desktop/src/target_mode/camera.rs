//! Independent free flight. Metre-scaled east/north; source vertical values.
#[derive(Clone, Debug)]
pub(crate) struct Camera {
    pub eye: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub speed: f32,
    pub looking: bool,
    pub positioned: bool,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            eye: [0., -1800., 1100.],
            yaw: 0.,
            pitch: -0.55,
            speed: 100.,
            looking: false,
            positioned: false,
        }
    }
}
impl Camera {
    pub fn reset(&mut self, elevation: f32) {
        self.eye = [0., -1800., elevation + 1100.];
        self.yaw = 0.;
        self.pitch = -0.55;
        self.positioned = true;
    }
    pub fn basis(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let (s, c) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        ([c, -s, 0.], [-s * sp, -c * sp, cp], [s * cp, c * cp, sp])
    }
    pub fn step(&mut self, movement: [f32; 3], mouse: egui::Vec2, dt: f32, fast: bool) {
        self.yaw = (self.yaw + mouse.x * 0.0025).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch - mouse.y * 0.0025).clamp(-1.55, 1.55);
        let (right, _, forward) = self.basis();
        let vector = std::array::from_fn::<_, 3, _>(|i| {
            right[i] * movement[0]
                + forward[i] * movement[1]
                + if i == 2 { movement[2] } else { 0. }
        });
        let length = vector.iter().map(|v| v * v).sum::<f32>().sqrt().max(1.);
        let scale = self.speed * dt.clamp(0., 0.05) * if fast { 4. } else { 1. } / length;
        for (pos, delta) in self.eye.iter_mut().zip(vector) {
            *pos += delta * scale;
        }
    }
    pub fn project(&self, point: [f32; 3], rect: egui::Rect) -> Option<egui::Pos2> {
        let (r, u, f) = self.basis();
        let d = std::array::from_fn::<_, 3, _>(|i| point[i] - self.eye[i]);
        let dot = |a: [f32; 3]| a.iter().zip(d).map(|(x, y)| x * y).sum::<f32>();
        let depth = dot(f);
        if depth <= 0.1 {
            return None;
        }
        let k = rect.height() / (2. * (60f32.to_radians() / 2.).tan() * depth);
        Some(rect.center() + egui::vec2(dot(r) * k, -dot(u) * k))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flight_uses_look_direction_vertical_controls_and_bounded_delta_time() {
        let mut c = Camera {
            eye: [0.; 3],
            pitch: 0.,
            ..Default::default()
        };
        c.step([0., 1., 0.], egui::Vec2::ZERO, 0.05, false);
        assert_eq!(c.eye, [0., 5., 0.]);
        c.step([0., 0., 1.], egui::Vec2::ZERO, 10., true);
        assert_eq!(c.eye, [0., 5., 20.]);
        c.yaw = std::f32::consts::FRAC_PI_2;
        c.step([0., 1., 0.], egui::Vec2::ZERO, 0.05, false);
        assert!((c.eye[0] - 5.).abs() < 1e-5);
        c.step([0.; 3], egui::vec2(0., 1e6), 0., false);
        assert_eq!(c.pitch, -1.55);
        let (r, u, f) = c.basis();
        for a in [r, u, f] {
            assert!((a.iter().map(|v| v * v).sum::<f32>() - 1.).abs() < 1e-5);
        }
    }
}
