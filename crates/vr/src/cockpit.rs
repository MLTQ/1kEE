use crate::{
    cartography,
    input::Hands,
    operator::Link,
    render::Renderer,
    scene::{Globe, pose},
    wrist,
};
use anyhow::{Result, bail};
use glam::{Vec2, Vec3};
use one_thousand_electric_eye_desktop::vr_operator::View;

#[derive(Clone, Copy)]
enum Hit {
    Surface(Vec3),
    Spire(usize),
    Wrist(Vec2),
    Brief,
}
fn aim(cart: &cartography::Cartography, globe: &Globe, aim: openxr::Posef) -> Option<(Vec3, Hit)> {
    let ray = pose(aim);
    let start = ray.w_axis.truncate();
    // The wrist is a foreground control surface, including when a user
    // lowers their hand inside the globe. Its beam uses the same priority.
    if let Some(wrist) = cart.wrist_pose.filter(|_| cart.wrist_ready)
        && let Some(uv) = wrist::hit(wrist, ray)
    {
        let point = wrist.transform_point3(glam::vec3(uv.x - 0.5, 0.5 - uv.y, 0.));
        return Some((point, Hit::Wrist(uv)));
    }
    let frame = cart.frame.as_ref();
    let (model, point, cursor) = if let Some(f) = frame.filter(|f| f.local) {
        let table = globe.table_transform();
        let uv = cartography::table_uv(f, table, ray);
        (
            table,
            uv.map(|p| glam::vec3(p.x - 0.5, 0.5 - p.y, cartography::height(f, p))),
            uv.map(|p| p.extend(0.)),
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
        let p = brief.transform_point3(glam::vec3(uv.x - 0.5, 0.5 - uv.y, 0.));
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
    menu: bool,
    pointer: wrist::Pointer,
    recenter: u64,
    exiting: bool,
}
impl Cockpit {
    pub fn new(link: Link, view: View) -> Self {
        Self {
            cursor: None,
            link,
            view,
            menu: true,
            pointer: Default::default(),
            recenter: 0,
            exiting: false,
        }
    }
    pub fn tick(
        &mut self,
        renderer: &mut Renderer,
        q: &wgpu::Queue,
        globe: &mut Globe,
        hands: &Hands,
        head: Vec3,
        dt: f32,
    ) -> Result<(bool, bool)> {
        let out = self.link.take();
        if let Some(e) = out.error {
            bail!("native operator failed: {e}");
        }
        let mut recenter = false;
        let mut exit = false;
        if let Some((revision, v, reset, quit)) = out.state {
            if revision == self.link.revision {
                self.view = v;
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
        cart.advance(q, out.map, out.wrist);
        cart.update_room(q, out.room, out.show_room);
        if hands.toggle {
            self.menu = !self.menu;
        }
        cart.wrist_pose = hands
            .left
            .filter(|_| self.menu)
            .map(|p| wrist::mount(pose(p), head));
        let hit = hands.right.and_then(|p| aim(cart, globe, p));
        let hover = match hit {
            Some((_, Hit::Wrist(uv))) => Some(uv),
            _ => None,
        };
        cart.pointer = hover;
        let mut events = self
            .pointer
            .events(hover, hands.down, hands.zoom * dt * 700.);
        if let Some(uv) = hover
            && hands.click
        {
            events.push(self.pointer.press(uv));
        }
        cart.aiming_at_target = matches!(hit, Some((_, Hit::Wrist(_) | Hit::Spire(_))));
        cart.laser = hands.right.map(|p| {
            let ray = pose(p);
            let start = ray.w_axis.truncate();
            (
                start,
                hit.map_or(start - ray.z_axis.truncate() * 2., |(end, _)| end),
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
            self.menu = true;
            self.link.pick(target);
        }
        if hover.is_none() {
            let previous = self.view;
            let moving = hands.rotate.abs().max_element() > 0.15 || hands.zoom.abs() > 0.15;
            if moving && self.view.local {
                let half = one_thousand_electric_eye_desktop::vr_operator::local_extent(
                    self.view.local_zoom,
                );
                let dead = |v: f32| if v.abs() > 0.15 { v } else { 0. };
                self.view.lat =
                    (self.view.lat + dead(hands.rotate.y) * dt * half).clamp(-89.9, 89.9);
                self.view.lon = (self.view.lon
                    + dead(hands.rotate.x) * dt * half / self.view.lat.to_radians().cos().max(0.2)
                    + 180.)
                    .rem_euclid(360.)
                    - 180.;
                self.view.local_zoom =
                    (self.view.local_zoom * (dead(hands.zoom) * dt).exp()).clamp(1., 60.);
            } else if moving {
                globe.rotate(hands.rotate.x, hands.rotate.y, dt);
                globe.zoom(hands.zoom, dt);
                self.view.lat = globe.lat;
                self.view.lon = globe.lon;
                self.view.scale = globe.scale;
            }
            if self.view != previous {
                self.link.navigate(self.view);
            }
        }
        self.link
            .wrist(events, cart.wrist_pose.is_some(), globe.capture(head));
        Ok((recenter, exit))
    }
}
