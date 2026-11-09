use crate::operator::WRIST_SIZE;
use glam::{Mat4, Vec2, Vec3};

pub fn mount(grip: Mat4, head: Vec3) -> Mat4 {
    // The anchor follows the back of the left hand. Face the user even when the wrist rolls.
    let center = grip.transform_point3(Vec3::new(0., 0.075, 0.09));
    let forward = (head - center).normalize_or(Vec3::Z);
    let right = Vec3::Y.cross(forward).normalize_or(Vec3::X);
    Mat4::from_cols(
        (right * 0.36).extend(0.),
        (forward.cross(right) * 0.30).extend(0.),
        forward.extend(0.),
        center.extend(1.),
    )
}
pub fn hit(panel: Mat4, ray: Mat4) -> Option<Vec2> {
    let m = panel.inverse() * ray;
    let origin = m.w_axis.truncate();
    let dir = -m.z_axis.truncate();
    if dir.z >= -1e-5 || origin.z < 0. {
        return None;
    }
    let p = origin - dir * (origin.z / dir.z);
    let uv = Vec2::new(0.5 + p.x, 0.5 - p.y);
    (uv.min_element() >= 0. && uv.max_element() <= 1.).then_some(uv)
}
#[derive(Default)]
pub struct Pointer {
    last: Option<Vec2>,
    captured: bool,
}
impl Pointer {
    pub fn events(&mut self, uv: Option<Vec2>, pressed: bool, scroll: f32) -> Vec<egui::Event> {
        let mut events = Vec::new();
        let mods = egui::Modifiers::NONE;
        let pos = |v: Vec2| egui::pos2(v.x * WRIST_SIZE[0] as f32, v.y * WRIST_SIZE[1] as f32);
        let button = |p, down| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: down,
            modifiers: mods,
        };
        if let Some(v) = uv {
            if self.last != uv {
                events.push(egui::Event::PointerMoved(pos(v)));
            }
            if !pressed && self.captured {
                events.push(button(pos(v), false));
                self.captured = false;
            }
            if scroll.abs() > 0.01 {
                events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., scroll),
                    modifiers: mods,
                });
            }
        } else if self.last.is_some() {
            if self.captured {
                let outside = egui::pos2(-200., -200.);
                events.push(egui::Event::PointerMoved(outside));
                events.push(button(outside, false));
                self.captured = false;
            }
            events.push(egui::Event::PointerGone);
        }
        self.last = uv;
        events
    }
    pub fn press(&mut self, uv: Vec2) -> egui::Event {
        self.captured = true;
        egui::Event::PointerButton {
            pos: egui::pos2(uv.x * WRIST_SIZE[0] as f32, uv.y * WRIST_SIZE[1] as f32),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn translated_wrist_picks_ui_coordinates_and_cancels_a_lost_press() {
        let panel = mount(
            Mat4::from_translation(Vec3::new(0.1, 0.2, -0.6)),
            Vec3::ZERO,
        );
        let target = panel.transform_point3(Vec3::new(0.25, 0.25, 0.));
        let normal = panel.z_axis.truncate();
        let ray = Mat4::look_to_rh(target + normal * 0.4, -normal, Vec3::Y).inverse();
        let uv = hit(panel, ray).unwrap();
        assert!(uv.distance(Vec2::new(0.75, 0.25)) < 1e-4);
        let mut p = Pointer::default();
        p.events(Some(uv), true, 0.);
        let _ = p.press(uv);
        let lost = p.events(None, true, 0.);
        assert!(matches!(
            lost.as_slice(),
            [
                egui::Event::PointerMoved(_),
                egui::Event::PointerButton { pressed: false, .. },
                egui::Event::PointerGone
            ]
        ));
        assert!(p.events(None, false, 0.).is_empty());
    }
}
