use anyhow::Result;
use openxr as xr;

pub struct Controls {
    set: xr::ActionSet,
    rotate: xr::Action<xr::Vector2f>,
    zoom: xr::Action<xr::Vector2f>,
    select: xr::Action<bool>,
    aim: xr::Space,
    grip: xr::Space,
    menu: xr::Action<bool>,
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
        let grip = set.create_action("left_wrist", "Left wrist", &[])?;
        let menu = set.create_action("wrist_menu", "Show wrist menu", &[])?;
        let path = |s| instance.string_to_path(s);
        instance.suggest_interaction_profile_bindings(
            path("/interaction_profiles/valve/index_controller")?,
            &[
                xr::Binding::new(&menu, path("/user/hand/left/input/a/click")?),
                xr::Binding::new(&grip, path("/user/hand/left/input/grip/pose")?),
                xr::Binding::new(&rotate, path("/user/hand/left/input/thumbstick")?),
                xr::Binding::new(&zoom, path("/user/hand/right/input/thumbstick")?),
                xr::Binding::new(&select, path("/user/hand/right/input/trigger/click")?),
                xr::Binding::new(&aim, path("/user/hand/right/input/aim/pose")?),
            ],
        )?;
        instance.suggest_interaction_profile_bindings(
            path("/interaction_profiles/khr/simple_controller")?,
            &[
                xr::Binding::new(&grip, path("/user/hand/left/input/grip/pose")?),
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
            grip: grip.create_space(session, xr::Path::NULL, xr::Posef::IDENTITY)?,
            menu,
            selected: false,
            cursor: None,
        })
    }
    pub fn sample(
        &self,
        session: &xr::Session<xr::Vulkan>,
        space: &xr::Space,
        time: xr::Time,
    ) -> Result<Hands> {
        session.sync_actions(&[(&self.set).into()])?;
        let rotate = self.rotate.state(session, xr::Path::NULL)?;
        let zoom = self.zoom.state(session, xr::Path::NULL)?;
        let trigger = self.select.state(session, xr::Path::NULL)?;
        let menu = self.menu.state(session, xr::Path::NULL)?;
        let locate = |s: &xr::Space| -> Result<Option<xr::Posef>> {
            let l = s.locate(space, time)?;
            Ok(l.location_flags
                .contains(
                    xr::SpaceLocationFlags::POSITION_VALID
                        | xr::SpaceLocationFlags::ORIENTATION_VALID,
                )
                .then_some(l.pose))
        };
        Ok(Hands {
            left: locate(&self.grip)?,
            right: locate(&self.aim)?,
            rotate: if rotate.is_active {
                glam::vec2(rotate.current_state.x, rotate.current_state.y)
            } else {
                glam::Vec2::ZERO
            },
            zoom: if zoom.is_active {
                zoom.current_state.y
            } else {
                0.
            },
            down: trigger.is_active && trigger.current_state,
            click: trigger.is_active && trigger.changed_since_last_sync && trigger.current_state,
            toggle: menu.is_active && menu.changed_since_last_sync && menu.current_state,
        })
    }
}
#[derive(Default)]
pub struct Hands {
    pub left: Option<xr::Posef>,
    pub right: Option<xr::Posef>,
    pub rotate: glam::Vec2,
    pub zoom: f32,
    pub down: bool,
    pub click: bool,
    pub toggle: bool,
}
