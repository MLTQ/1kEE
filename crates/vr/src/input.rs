use crate::scene::{Globe, Marker};
use anyhow::Result;
use openxr as xr;

pub struct Controls {
    set: xr::ActionSet,
    rotate: xr::Action<xr::Vector2f>,
    zoom: xr::Action<xr::Vector2f>,
    select: xr::Action<bool>,
    aim: xr::Space,
    pub selected: bool,
    pub cursor: Option<glam::Vec3>,
}
impl Controls {
    pub fn new(instance: &xr::Instance, session: &xr::Session<xr::Vulkan>) -> Result<Self> {
        let set = instance.create_action_set("globe", "Globe", 0)?;
        let rotate = set.create_action("rotate", "Rotate globe", &[])?;
        let zoom = set.create_action("zoom", "Zoom globe", &[])?;
        let select = set.create_action("select", "Select marker", &[])?;
        let aim = set.create_action("aim", "Point at marker", &[])?;
        let path = |s| instance.string_to_path(s);
        instance.suggest_interaction_profile_bindings(
            path("/interaction_profiles/valve/index_controller")?,
            &[
                xr::Binding::new(&rotate, path("/user/hand/left/input/thumbstick")?),
                xr::Binding::new(&zoom, path("/user/hand/right/input/thumbstick")?),
                xr::Binding::new(&select, path("/user/hand/right/input/trigger/click")?),
                xr::Binding::new(&aim, path("/user/hand/right/input/aim/pose")?),
            ],
        )?;
        instance.suggest_interaction_profile_bindings(
            path("/interaction_profiles/khr/simple_controller")?,
            &[
                xr::Binding::new(&select, path("/user/hand/right/input/select/click")?),
                xr::Binding::new(&aim, path("/user/hand/right/input/aim/pose")?),
            ],
        )?;
        session.attach_action_sets(&[&set])?;
        let aim_space = aim.create_space(session, xr::Path::NULL, xr::Posef::IDENTITY)?;
        Ok(Self {
            set,
            rotate,
            zoom,
            select,
            aim: aim_space,
            selected: false,
            cursor: None,
        })
    }
    pub fn update(
        &mut self,
        session: &xr::Session<xr::Vulkan>,
        space: &xr::Space,
        time: xr::Time,
        globe: &mut Globe,
        marker: &Marker,
        dt: f32,
    ) -> Result<()> {
        session.sync_actions(&[(&self.set).into()])?;
        let rotate = self.rotate.state(session, xr::Path::NULL)?;
        if rotate.is_active {
            globe.rotate(rotate.current_state.x, rotate.current_state.y, dt);
        }
        let zoom = self.zoom.state(session, xr::Path::NULL)?;
        if zoom.is_active {
            globe.zoom(zoom.current_state.y, dt);
        }
        let location = self.aim.locate(space, time)?;
        self.cursor = if location.location_flags.contains(
            xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID,
        ) {
            globe.aim_point(location.pose)
        } else {
            None
        };
        let trigger = self.select.state(session, xr::Path::NULL)?;
        if trigger.is_active && trigger.changed_since_last_sync && trigger.current_state {
            self.selected = self.cursor.is_some_and(|p| {
                p.distance(crate::scene::surface(marker.lat, marker.lon)) * globe.scale < 0.045
            });
            if self.selected {
                println!("Selected: {}", marker.title);
            }
        }
        Ok(())
    }
}
