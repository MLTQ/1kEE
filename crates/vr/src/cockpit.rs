//! VR cockpit controller: Half-Life: Alyx–style look-to-wake forearm HUD,
//! direct 1-hand grab pan & 2-hand pinch/spread continuous zoom (24,000 km → 20 m),
//! spire picking, and OpenXR haptic cues.
use crate::{
    cartography,
    input::Hands,
    operator::Link,
    render::Renderer,
    scene::{Globe, RADIUS_M, pose},
    wrist::{self, WristVisibility},
};
use anyhow::{Result, bail};
use glam::{Vec2, Vec3};
use one_thousand_electric_eye_desktop::vr_operator::{
    MAX_RANGE_M, MIN_RANGE_M, View, VrCommand, VrSnapshot,
};

#[derive(Clone, Copy)]
enum Hit {
    Surface(Vec3),
    Spire(usize),
    Wrist(Vec2),
    Brief,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct HapticEvent {
    pub left: Option<(f32, i64)>,
    pub right: Option<(f32, i64)>,
}

#[derive(Clone, Copy, Debug)]
enum GrabState {
    None,
    OneHand { right: bool, last_pos: Vec3 },
    TwoHand { last_dist: f32, last_mid: Vec3 },
}

fn aim(cart: &cartography::Cartography, globe: &Globe, aim: openxr::Posef) -> Option<(Vec3, Hit)> {
    let ray = pose(aim);
    let start = ray.w_axis.truncate();
    // The wrist HUD is a foreground control surface whenever it is awake.
    if let Some(wrist) = cart.wrist_pose.filter(|_| cart.wrist_alpha >= 0.15)
        && let Some(uv) = wrist::hit(wrist, ray)
    {
        let point = wrist.transform_point3(glam::vec3(uv.x - 0.5, 0.5 - uv.y, 0.0));
        return Some((point, Hit::Wrist(uv)));
    }
    let frame = cart.frame.as_ref();
    let (model, point, cursor) = if let Some(f) = frame.filter(|f| f.local) {
        let table = globe.table_transform();
        let uv = cartography::table_uv(f, table, ray);
        (
            table,
            uv.map(|p| glam::vec3(p.x - 0.5, 0.5 - p.y, cartography::height(f, p))),
            uv.map(|p| p.extend(0.0)),
        )
    } else {
        let p = globe.aim_point(aim);
        (globe.transform(), p, p)
    };
    let local = model.inverse() * ray;
    let origin = local.w_axis.truncate();
    let mut nearest = point
        .zip(cursor)
        .map(|(p, c)| (model.transform_point3(p), Hit::Surface(c)));
    if let Some((i, t)) = frame.and_then(|f| {
        cart.spires.pick(
            local,
            point.map_or(f32::INFINITY, |p| p.distance(origin)),
            f,
        )
    }) {
        let p = origin - local.z_axis.truncate().normalize() * t;
        nearest = Some((model.transform_point3(p), Hit::Spire(i)));
    }
    let brief = globe.brief_transform();
    if cart.show_room
        && cart.room_ready
        && frame.is_some()
        && let Some(uv) = wrist::hit(brief, ray)
    {
        let p = brief.transform_point3(glam::vec3(uv.x - 0.5, 0.5 - uv.y, 0.0));
        if nearest
            .is_none_or(|(other, _)| start.distance_squared(p) <= start.distance_squared(other))
        {
            nearest = Some((p, Hit::Brief));
        }
    }
    nearest
}

pub struct Cockpit {
    pub cursor: Option<Vec3>,
    link: Link,
    view: View,
    snapshot: VrSnapshot,
    visibility: WristVisibility,
    pointer: wrist::Pointer,
    grab: GrabState,
    last_hovered_spire: Option<usize>,
    recenter: u64,
    exiting: bool,
}

impl Cockpit {
    pub fn new(link: Link, view: View) -> Self {
        let mut snapshot = VrSnapshot::default();
        snapshot.view = view;
        Self {
            cursor: None,
            link,
            view,
            snapshot,
            visibility: WristVisibility::default(),
            pointer: Default::default(),
            grab: GrabState::None,
            last_hovered_spire: None,
            recenter: 0,
            exiting: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut Renderer,
        q: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        globe: &mut Globe,
        hands: &Hands,
        head: Vec3,
        dt: f32,
    ) -> Result<(bool, bool, HapticEvent)> {
        let mut haptics = HapticEvent::default();
        let out = self.link.take();
        if let Some(e) = out.error {
            bail!("native operator failed: {e}");
        }
        let mut recenter = false;
        let mut exit = false;
        if let Some(snap) = out.snapshot {
            let local_view = self.view;
            self.snapshot = snap;
            // Preserve live grab/stick view until operator catches up with our revision.
            self.snapshot.view = local_view;
        }
        if let Some((revision, v, reset, quit)) = out.state {
            if revision == self.link.revision {
                self.view = v;
                self.snapshot.view = v;
                globe.lat = v.lat;
                globe.lon = v.lon;
                globe.scale = v.scale;
            }
            if reset != self.recenter {
                self.recenter = reset;
                recenter = true;
            }
            if quit && !self.exiting {
                self.exiting = true;
                exit = true;
            }
        }

        let cart = &mut renderer.cartography;
        cart.advance(q, out.map);
        cart.update_room(q, out.room, out.show_room);

        if hands.toggle {
            self.visibility.toggle_pin();
            haptics.left = Some((0.28, 16));
        }

        // Compute compact Alyx forearm mount and look-to-wake alpha.
        let mount = hands.left.map(|p| wrist::mount_wrist(pose(p), head));
        let pre_aiming_wrist = mount
            .zip(hands.right)
            .is_some_and(|(m, r)| wrist::hit(m.transform, pose(r)).is_some());
        let alpha = self.visibility.update(mount, pre_aiming_wrist, dt);
        cart.wrist_alpha = alpha;
        cart.wrist_pose = mount.filter(|_| alpha > 0.01).map(|m| m.transform);

        let hit = hands.right.and_then(|p| aim(cart, globe, p));
        let raw_hover = match hit {
            Some((_, Hit::Wrist(uv))) if self.visibility.is_interactive() => Some(uv),
            _ => None,
        };
        let events = self
            .pointer
            .events(raw_hover, hands.down, hands.click, hands.zoom * dt * 260.0);
        let hover = self.pointer.stabilized_uv();
        cart.pointer = hover;

        // Render the direct-GPU wrist HUD straight into the headset command encoder
        // (zero CPU readback, full 90/120 Hz responsiveness).
        let mut commands = Vec::new();
        if alpha > 0.01 {
            let (hud_cmds, fb) = renderer.wrist_hud.render(
                device,
                q,
                encoder,
                events,
                &mut self.snapshot,
                self.visibility.pinned,
            );
            if fb.toggle_pin {
                self.visibility.toggle_pin();
            }
            if fb.click_tick {
                haptics.right = Some((0.45, 22));
            } else if fb.hover_tick {
                haptics.right = Some((0.16, 10));
            }
            for cmd in hud_cmds {
                match &cmd {
                    VrCommand::Recenter => recenter = true,
                    VrCommand::Exit => {
                        if !self.exiting {
                            self.exiting = true;
                            exit = true;
                        }
                    }
                    VrCommand::SetRange(r) => {
                        self.view = View::from_continuous(self.view.lat, self.view.lon, *r);
                        globe.scale = self.view.scale;
                        self.link.navigate(self.view);
                    }
                    _ => commands.push(cmd),
                }
            }
        }

        let cart = &mut renderer.cartography;
        let hovered_spire = match hit {
            Some((_, Hit::Spire(i))) => Some(i),
            _ => None,
        };
        if hovered_spire.is_some() && hovered_spire != self.last_hovered_spire {
            haptics.right = haptics.right.or(Some((0.20, 12)));
        }
        self.last_hovered_spire = hovered_spire;

        cart.aiming_at_target = matches!(hit, Some((_, Hit::Wrist(_) | Hit::Spire(_))));
        cart.laser = hands.right.map(|p| {
            let ray = pose(p);
            let start = ray.w_axis.truncate();
            (
                start,
                hit.map_or(start - ray.z_axis.truncate() * 2.0, |(end, _)| end),
            )
        });
        self.cursor = match hit {
            Some((_, Hit::Surface(p))) => Some(p),
            _ => None,
        };

        if hands.click
            && let Some((_, Hit::Spire(i))) = hit
            && let Some(target) = cart.frame.as_ref().and_then(|f| f.spires[i].target.clone())
        {
            self.link.pick(target);
            haptics.right = Some((0.50, 24));
        }

        // Direct physical 1-hand grab pan / 2-hand pinch-spread continuous zoom
        // + smooth thumbstick fallback whenever the right laser isn't over the wrist HUD.
        let previous = self.view;
        if hover.is_none() {
            let grabbed = self.apply_grab_navigation(globe, hands, &mut haptics);
            if !grabbed {
                self.apply_thumbstick_navigation(globe, hands, dt);
            }
        } else {
            self.grab = GrabState::None;
        }

        if self.view != previous {
            self.snapshot.view = self.view;
            globe.lat = self.view.lat;
            globe.lon = self.view.lon;
            globe.scale = self.view.scale;
            self.link.navigate(self.view);
        }

        self.link.send_commands(commands, globe.capture(head));
        Ok((recenter, exit, haptics))
    }

    fn apply_grab_navigation(
        &mut self,
        globe: &Globe,
        hands: &Hands,
        haptics: &mut HapticEvent,
    ) -> bool {
        let l_pos = hands.left.filter(|_| hands.left_grab).map(|p| pose(p).w_axis.truncate());
        let r_pos = hands
            .right_grip
            .filter(|_| hands.right_grab)
            .map(|p| pose(p).w_axis.truncate());

        if hands.left_grab_start {
            haptics.left = haptics.left.or(Some((0.32, 18)));
        }
        if hands.right_grab_start {
            haptics.right = haptics.right.or(Some((0.32, 18)));
        }

        match (l_pos, r_pos) {
            (Some(lp), Some(rp)) => {
                let dist = lp.distance(rp).max(0.04);
                let mid = (lp + rp) * 0.5;
                if let GrabState::TwoHand {
                    last_dist,
                    last_mid,
                } = self.grab
                {
                    let ratio = (last_dist / dist).clamp(0.75, 1.35) as f64;
                    let next_range = (self.view.range_m * ratio).clamp(MIN_RANGE_M, MAX_RANGE_M);
                    let delta = mid - last_mid;
                    let (dlat, dlon) = physical_pan_deg(globe, &self.view, delta);
                    self.view = View::from_continuous(
                        self.view.lat + dlat,
                        self.view.lon + dlon,
                        next_range,
                    );
                }
                self.grab = GrabState::TwoHand {
                    last_dist: dist,
                    last_mid: mid,
                };
                true
            }
            (Some(pos), None) => {
                if let GrabState::OneHand {
                    right: false,
                    last_pos,
                } = self.grab
                {
                    let (dlat, dlon) = physical_pan_deg(globe, &self.view, pos - last_pos);
                    self.view = View::from_continuous(
                        self.view.lat + dlat,
                        self.view.lon + dlon,
                        self.view.range_m,
                    );
                }
                self.grab = GrabState::OneHand {
                    right: false,
                    last_pos: pos,
                };
                true
            }
            (None, Some(pos)) => {
                if let GrabState::OneHand {
                    right: true,
                    last_pos,
                } = self.grab
                {
                    let (dlat, dlon) = physical_pan_deg(globe, &self.view, pos - last_pos);
                    self.view = View::from_continuous(
                        self.view.lat + dlat,
                        self.view.lon + dlon,
                        self.view.range_m,
                    );
                }
                self.grab = GrabState::OneHand {
                    right: true,
                    last_pos: pos,
                };
                true
            }
            (None, None) => {
                self.grab = GrabState::None;
                false
            }
        }
    }

    fn apply_thumbstick_navigation(&mut self, _globe: &Globe, hands: &Hands, dt: f32) {
        let dead = |v: f32| if v.abs() > 0.15 { v } else { 0.0 };
        let rx = dead(hands.rotate.x);
        let ry = dead(hands.rotate.y);
        let rz = dead(hands.zoom);
        if rx == 0.0 && ry == 0.0 && rz == 0.0 {
            return;
        }
        // Continuous logarithmic zoom across the entire planet → street span.
        let next_range =
            (self.view.range_m * (-(rz as f64) * dt as f64 * 1.65).exp()).clamp(MIN_RANGE_M, MAX_RANGE_M);
        // Scale angular pan speed cleanly to current visible ground span.
        let span_deg = ((next_range / 111_320.0).min(110.0) as f32).max(1e-4);
        let cos_lat = self.view.lat.to_radians().cos().abs().max(0.2);
        let dlat = ry * dt * span_deg * 0.65;
        let dlon = rx * dt * span_deg * 0.65 / cos_lat;
        self.view = View::from_continuous(self.view.lat + dlat, self.view.lon + dlon, next_range);
    }
}

/// Convert a physical 3D controller displacement (metres in world space) into
/// natural map pan deltas `(dlat, dlon)` in degrees, feeling 1:1 on both the
/// orbital globe and the close-up holographic relief table.
fn physical_pan_deg(globe: &Globe, view: &View, world_delta: Vec3) -> (f32, f32) {
    let cos_lat = view.lat.to_radians().cos().abs().max(0.2);
    if view.local {
        // Transform displacement into table-width units (`0.85 m` == visible span).
        let local = globe.table_transform().inverse().transform_vector3(world_delta);
        let span_lat = (view.range_m / 111_320.0) as f32;
        let span_lon = span_lat / cos_lat;
        (-local.y * span_lat, -local.x * span_lon)
    } else {
        // Trackball grab on the physical sphere of radius `RADIUS_M * scale`.
        let local = globe.heading().inverse() * world_delta;
        let radius_m = (RADIUS_M * view.scale.max(1.0)).max(0.15);
        let rad_per_m = 1.0 / radius_m;
        let dlat = -(local.y * rad_per_m).to_degrees();
        let dlon = -(local.x * rad_per_m).to_degrees() / cos_lat;
        (dlat, dlon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_vr_zoom_spans_orbit_to_street_smoothly() {
        let orbit = View::from_continuous(46.8523, -121.7603, 18_000_000.0);
        assert!(!orbit.local);
        assert!(orbit.scale >= 1.0 && orbit.scale < 2.0);

        let regional = View::from_continuous(46.8523, -121.7603, 500_000.0);
        assert!(!regional.local);
        assert!(regional.scale > orbit.scale);

        let city = View::from_continuous(46.8523, -121.7603, 25_000.0);
        assert!(city.local);

        let street = View::from_continuous(46.8523, -121.7603, 50.0);
        assert!(street.local);
        assert!(street.local_zoom > city.local_zoom);
        assert_eq!(street.scale_label(), "50 m");
    }
}
