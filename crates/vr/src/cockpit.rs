use crate::{
    cartography,
    input::Hands,
    operator::Link,
    render::Renderer,
    scene::{Globe, pose},
    wrist,
};
use anyhow::{Result, bail};
use glam::Vec3;
use one_thousand_electric_eye_desktop::vr_operator::View;

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
        let ray = hands.right.map(pose);
        let hover = ray
            .zip(cart.wrist_pose)
            .and_then(|(r, p)| wrist::hit(p, r))
            .filter(|_| cart.wrist_ready);
        cart.pointer = hover;
        let mut events = self
            .pointer
            .events(hover, hands.down, hands.zoom * dt * 700.);
        if let Some(uv) = hover
            && hands.click
        {
            events.push(self.pointer.press(uv));
        }
        self.link.wrist(events, cart.wrist_pose.is_some());
        self.cursor = None;
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
            if let (Some(f), Some(r)) = (&cart.frame, ray) {
                let uv = if f.local {
                    let p = cartography::table_uv(f, globe.table_transform(), r);
                    self.cursor = p.map(|uv| uv.extend(0.));
                    p
                } else {
                    self.cursor = hands.right.and_then(|p| globe.aim_point(p));
                    self.cursor.and_then(|p| cartography::globe_uv(f, p))
                };
                if hands.click
                    && let Some(target) = uv.and_then(|v| f.hit(v.to_array()))
                {
                    self.menu = true;
                    self.link.pick(target);
                }
            }
        }
        Ok((recenter, exit))
    }
}
