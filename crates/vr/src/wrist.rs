//! Half-Life: Alyx–style forearm/wrist HUD mounting, look-to-wake visibility,
//! and trigger-stabilized laser pointer interaction.
use glam::{Mat4, Vec2, Vec3};

/// Backing GPU texture size for the direct-rendered wrist HUD (pixels).
pub const HUD_SIZE: [u32; 2] = [640, 480];
/// Logical scale factor (2.0 → crisp 320 × 240 logical points in VR).
pub const HUD_DPI: f32 = 2.0;
/// Logical egui viewport dimensions (points).
pub const HUD_LOGICAL: [f32; 2] = [HUD_SIZE[0] as f32 / HUD_DPI, HUD_SIZE[1] as f32 / HUD_DPI];

/// Compact forearm gauntlet physical size in metres (17.5 cm × 13.2 cm).
pub const PANEL_WIDTH_M: f32 = 0.175;
pub const PANEL_HEIGHT_M: f32 = 0.132;

/// Mount result carrying both the world-space quad transform and view geometry
/// needed for look-to-wake.
#[derive(Clone, Copy, Debug)]
pub struct WristMount {
    /// World transform scaling the unit quad `[-0.5, 0.5]²` to physical metres.
    pub transform: Mat4,
    /// Dot product between the panel's outward normal and the vector toward the head.
    pub facing: f32,
    /// Distance in metres from the panel center to the user's head.
    pub distance: f32,
}

/// Mount the compact HUD physically onto the left forearm/inner wrist.
///
/// In OpenXR left-hand grip space: `-Z` points forward toward the fingers,
/// `+Z` points back along the forearm toward the elbow, `+Y` points up out of
/// the wrist/back of hand, and `+X` points inward toward the body.
///
/// When the user raises their left forearm across their chest like checking a
/// watch or Alyx gauntlet, the panel rests cleanly along the wrist, reads left
/// to right from elbow to fingers, and uses only a subtle ergonomic tilt
/// assist (~15° max) so it stays physically anchored to the arm rather than
/// billboarding wildly.
pub fn mount_wrist(grip: Mat4, head: Vec3) -> WristMount {
    let center = grip.transform_point3(Vec3::new(0.016, 0.052, 0.092));
    let to_head = (head - center).normalize_or(Vec3::Z);
    let distance = head.distance(center);

    // Physical outward normal out of the top/dorsal wrist, angled slightly
    // toward the user's face when the left forearm is raised across the chest.
    let base_normal = grip
        .transform_vector3(Vec3::new(-0.15, 0.96, -0.22))
        .normalize_or(Vec3::Y);
    let facing = base_normal.dot(to_head);

    // Subtle look-assist (up to ~15°) when already roughly facing the user so
    // small wrist-angle differences between Index/Touch grips feel effortless.
    let assist = ((facing - 0.25) / 0.75).clamp(0.0, 1.0) * 0.25;
    let normal = base_normal.lerp(to_head, assist).normalize_or(base_normal);

    // Text runs left-to-right along the forearm from elbow (+Z) to fingers (-Z).
    let raw_right = -grip.z_axis.truncate().normalize_or(Vec3::X);
    let right_proj = raw_right - normal * raw_right.dot(normal);
    let right = if right_proj.length_squared() > 1e-6 {
        right_proj.normalize()
    } else {
        Vec3::Y.cross(normal).normalize_or(Vec3::X)
    };
    let up = normal.cross(right).normalize_or(Vec3::Y);

    let transform = Mat4::from_cols(
        (right * PANEL_WIDTH_M).extend(0.0),
        (up * PANEL_HEIGHT_M).extend(0.0),
        normal.extend(0.0),
        center.extend(1.0),
    );
    WristMount {
        transform,
        facing,
        distance,
    }
}

/// Convenience helper returning just the panel transform (used by tests).
#[cfg(test)]
pub fn mount(grip: Mat4, head: Vec3) -> Mat4 {
    mount_wrist(grip, head).transform
}

/// Look-to-wake & pin visibility controller with smooth opacity transitions.
pub struct WristVisibility {
    pub pinned: bool,
    awake: bool,
    pub alpha: f32,
}

impl Default for WristVisibility {
    fn default() -> Self {
        Self {
            pinned: false,
            awake: false,
            alpha: 0.0,
        }
    }
}

impl WristVisibility {
    pub fn toggle_pin(&mut self) {
        self.pinned = !self.pinned;
        if self.pinned {
            self.awake = true;
        }
    }

    /// Update smooth opacity (`0.0..=1.0`) from the wrist mount angle, distance,
    /// and whether the right-hand laser is currently interacting with the HUD.
    pub fn update(&mut self, mount: Option<WristMount>, aiming_at_wrist: bool, dt: f32) -> f32 {
        let Some(m) = mount else {
            self.awake = false;
            self.alpha = (self.alpha - dt * 10.0).max(0.0);
            return self.alpha;
        };

        let target_awake = if self.pinned {
            true
        } else if self.awake {
            // Generous hysteresis so glancing slightly away or interacting
            // doesn't collapse the HUD mid-click.
            (m.facing >= 0.24 && m.distance <= 1.05) || (aiming_at_wrist && m.facing >= 0.05)
        } else {
            // Wake cleanly when the user lifts their forearm toward their eyes.
            m.facing >= 0.44 && m.distance <= 0.88
        };
        self.awake = target_awake;

        let target_alpha = if self.awake { 1.0 } else { 0.0 };
        let speed = if target_alpha > self.alpha { 9.0 } else { 6.5 };
        let step = (dt * speed).clamp(0.0, 1.0);
        self.alpha += (target_alpha - self.alpha) * step;
        if (self.alpha - target_alpha).abs() < 0.01 {
            self.alpha = target_alpha;
        }
        self.alpha
    }

    pub fn is_interactive(&self) -> bool {
        self.alpha >= 0.15
    }
}

/// Ray–quad intersection in panel UV space `[0, 1]²`.
pub fn hit(panel: Mat4, ray: Mat4) -> Option<Vec2> {
    let m = panel.inverse() * ray;
    let origin = m.w_axis.truncate();
    let dir = -m.z_axis.truncate();
    if dir.z >= -1e-5 || origin.z < 0.0 {
        return None;
    }
    let p = origin - dir * (origin.z / dir.z);
    let uv = Vec2::new(0.5 + p.x, 0.5 - p.y);
    (uv.min_element() >= 0.0 && uv.max_element() <= 1.0).then_some(uv)
}

/// Trigger-stabilized laser pointer translating UV hits into logical egui events.
#[derive(Default)]
pub struct Pointer {
    last: Option<Vec2>,
    smoothed: Option<Vec2>,
    press_anchor: Option<Vec2>,
    captured: bool,
}

impl Pointer {
    fn to_pos(v: Vec2) -> egui::Pos2 {
        egui::pos2(v.x * HUD_LOGICAL[0], v.y * HUD_LOGICAL[1])
    }

    pub fn stabilized_uv(&self) -> Option<Vec2> {
        self.smoothed
    }

    pub fn events(
        &mut self,
        uv: Option<Vec2>,
        pressed: bool,
        clicked: bool,
        scroll: f32,
    ) -> Vec<egui::Event> {
        let mut events = Vec::new();
        let mods = egui::Modifiers::NONE;
        let button = |p, down| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: down,
            modifiers: mods,
        };

        if let Some(raw) = uv {
            // Smooth laser jitter lightly; while pressing the trigger, hold a
            // small deadband around the press point so squeezing doesn't slide
            // off the button.
            let filtered = match self.smoothed {
                Some(prev) => {
                    let alpha = if pressed { 0.35 } else { 0.65 };
                    prev.lerp(raw, alpha)
                }
                None => raw,
            };
            let effective = if pressed
                && let Some(anchor) = self.press_anchor
                && filtered.distance(anchor) < 0.018
            {
                anchor
            } else {
                filtered
            };
            self.smoothed = Some(effective);

            if self.last != Some(effective) {
                events.push(egui::Event::PointerMoved(Self::to_pos(effective)));
            }
            if clicked {
                self.captured = true;
                self.press_anchor = Some(effective);
                events.push(button(Self::to_pos(effective), true));
            } else if !pressed && self.captured {
                events.push(button(Self::to_pos(effective), false));
                self.captured = false;
                self.press_anchor = None;
            }
            if scroll.abs() > 0.01 {
                events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, scroll),
                    modifiers: mods,
                });
            }
            self.last = Some(effective);
        } else if self.last.is_some() {
            if self.captured {
                let outside = egui::pos2(-200.0, -200.0);
                events.push(egui::Event::PointerMoved(outside));
                events.push(button(outside, false));
                self.captured = false;
                self.press_anchor = None;
            }
            events.push(egui::Event::PointerGone);
            self.last = None;
            self.smoothed = None;
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forearm_mount_is_compact_right_handed_and_hits_uv_cleanly() {
        // Left forearm raised across chest like checking a wristwatch:
        // fingers (-Z) point right (+X), wrist top (+Y) faces eyes (+Z), palm (+X) faces down (-Y).
        let grip = Mat4::from_cols(
            Vec3::NEG_Y.extend(0.0),
            Vec3::Z.extend(0.0),
            Vec3::NEG_X.extend(0.0),
            Vec3::new(-0.08, -0.16, -0.42).extend(1.0),
        );
        let wm = mount_wrist(grip, Vec3::ZERO);

        // Physical dimensions match the compact Alyx gauntlet (not the old 36×30 cm board).
        assert!((wm.transform.x_axis.truncate().length() - PANEL_WIDTH_M).abs() < 1e-4);
        assert!((wm.transform.y_axis.truncate().length() - PANEL_HEIGHT_M).abs() < 1e-4);
        // Right-handed basis (no mirrored text).
        let det = glam::Mat3::from_cols(
            wm.transform.x_axis.truncate().normalize(),
            wm.transform.y_axis.truncate().normalize(),
            wm.transform.z_axis.truncate().normalize(),
        )
        .determinant();
        assert!(det > 0.99, "wrist basis must be right-handed, det={det}");
        assert!(wm.facing > 0.6, "raised wrist should face head: {}", wm.facing);

        let target = wm.transform.transform_point3(Vec3::new(0.25, 0.25, 0.0));
        let normal = wm.transform.z_axis.truncate();
        let ray = Mat4::look_to_rh(target + normal * 0.4, -normal, Vec3::Y).inverse();
        let uv = hit(wm.transform, ray).unwrap();
        assert!(uv.distance(Vec2::new(0.75, 0.25)) < 1e-4);
    }

    #[test]
    fn look_to_wake_wakes_on_wrist_raise_and_sleeps_when_lowered() {
        let mut vis = WristVisibility::default();
        let awake_mount = WristMount {
            transform: Mat4::IDENTITY,
            facing: 0.75,
            distance: 0.45,
        };
        for _ in 0..20 {
            vis.update(Some(awake_mount), false, 0.05);
        }
        assert!(vis.alpha > 0.95 && vis.is_interactive());

        let lowered_mount = WristMount {
            transform: Mat4::IDENTITY,
            facing: -0.2,
            distance: 0.75,
        };
        for _ in 0..30 {
            vis.update(Some(lowered_mount), false, 0.05);
        }
        assert!(vis.alpha < 0.05 && !vis.is_interactive());

        // Pinning keeps it awake even when lowered.
        vis.toggle_pin();
        for _ in 0..20 {
            vis.update(Some(lowered_mount), false, 0.05);
        }
        assert!(vis.alpha > 0.95 && vis.is_interactive());
    }

    #[test]
    fn pointer_stabilizes_click_and_cancels_lost_press() {
        let mut p = Pointer::default();
        let uv = Vec2::new(0.5, 0.5);
        let e1 = p.events(Some(uv), true, true, 0.0);
        assert!(e1.iter().any(|e| matches!(
            e,
            egui::Event::PointerButton { pressed: true, .. }
        )));
        // Slight jitter during trigger hold stays pinned to press anchor.
        let _ = p.events(Some(uv + Vec2::new(0.005, -0.004)), true, false, 0.0);
        assert!(p.stabilized_uv().unwrap().distance(uv) < 1e-4);

        let lost = p.events(None, true, false, 0.0);
        assert!(matches!(
            lost.as_slice(),
            [
                egui::Event::PointerMoved(_),
                egui::Event::PointerButton { pressed: false, .. },
                egui::Event::PointerGone
            ]
        ));
        assert!(p.events(None, false, false, 0.0).is_empty());
    }
}
