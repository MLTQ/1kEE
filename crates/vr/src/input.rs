//! OpenXR controller bindings, dual-hand grip grab, and haptic feedback
//! supporting Valve Index, Oculus/Meta Touch, and KHR Simple controllers.
use anyhow::Result;
use openxr as xr;

pub struct Controls {
    set: xr::ActionSet,
    rotate: xr::Action<xr::Vector2f>,
    zoom: xr::Action<xr::Vector2f>,
    select: xr::Action<bool>,
    trigger: xr::Action<f32>,
    left_squeeze: xr::Action<f32>,
    right_squeeze: xr::Action<f32>,
    trigger_down: bool,
    left_grab_down: bool,
    right_grab_down: bool,
    aim: xr::Space,
    left_grip: xr::Space,
    right_grip: xr::Space,
    menu: xr::Action<bool>,
    haptic_left: xr::Action<xr::Haptic>,
    haptic_right: xr::Action<xr::Haptic>,
    pub selected: bool,
    pub cursor: Option<glam::Vec3>,
}

impl Controls {
    pub fn new(instance: &xr::Instance, session: &xr::Session<xr::Vulkan>) -> Result<Self> {
        let set = instance.create_action_set("globe", "1kEE VR Controls", 0)?;
        let rotate = set.create_action("rotate", "Orbit / pan map", &[])?;
        let zoom = set.create_action("zoom", "Zoom map / scroll HUD", &[])?;
        let select = set.create_action("select", "Select", &[])?;
        let trigger = set.create_action("trigger_pull", "Right trigger pull", &[])?;
        let left_squeeze = set.create_action("left_grab", "Left hand grab", &[])?;
        let right_squeeze = set.create_action("right_grab", "Right hand grab", &[])?;
        let aim = set.create_action("aim", "Right pointer aim", &[])?;
        let left_grip_act = set.create_action("left_wrist", "Left wrist / grip", &[])?;
        let right_grip_act = set.create_action("right_grip", "Right grip", &[])?;
        let menu = set.create_action("wrist_menu", "Pin wrist HUD", &[])?;
        let haptic_left = set.create_action("haptic_left", "Left haptic feedback", &[])?;
        let haptic_right = set.create_action("haptic_right", "Right haptic feedback", &[])?;

        let path = |s| instance.string_to_path(s);

        // Valve Index controllers
        let _ = instance.suggest_interaction_profile_bindings(
            path("/interaction_profiles/valve/index_controller")?,
            &[
                xr::Binding::new(&menu, path("/user/hand/left/input/a/click")?),
                xr::Binding::new(&left_grip_act, path("/user/hand/left/input/grip/pose")?),
                xr::Binding::new(&right_grip_act, path("/user/hand/right/input/grip/pose")?),
                xr::Binding::new(&left_squeeze, path("/user/hand/left/input/squeeze/value")?),
                xr::Binding::new(&right_squeeze, path("/user/hand/right/input/squeeze/value")?),
                xr::Binding::new(&rotate, path("/user/hand/left/input/thumbstick")?),
                xr::Binding::new(&zoom, path("/user/hand/right/input/thumbstick")?),
                xr::Binding::new(&trigger, path("/user/hand/right/input/trigger/value")?),
                xr::Binding::new(&aim, path("/user/hand/right/input/aim/pose")?),
                xr::Binding::new(&haptic_left, path("/user/hand/left/output/haptic")?),
                xr::Binding::new(&haptic_right, path("/user/hand/right/output/haptic")?),
            ],
        );

        // Oculus / Meta Touch controllers
        let _ = instance.suggest_interaction_profile_bindings(
            path("/interaction_profiles/oculus/touch_controller")?,
            &[
                xr::Binding::new(&menu, path("/user/hand/left/input/x/click")?),
                xr::Binding::new(&left_grip_act, path("/user/hand/left/input/grip/pose")?),
                xr::Binding::new(&right_grip_act, path("/user/hand/right/input/grip/pose")?),
                xr::Binding::new(&left_squeeze, path("/user/hand/left/input/squeeze/value")?),
                xr::Binding::new(&right_squeeze, path("/user/hand/right/input/squeeze/value")?),
                xr::Binding::new(&rotate, path("/user/hand/left/input/thumbstick")?),
                xr::Binding::new(&zoom, path("/user/hand/right/input/thumbstick")?),
                xr::Binding::new(&trigger, path("/user/hand/right/input/trigger/value")?),
                xr::Binding::new(&aim, path("/user/hand/right/input/aim/pose")?),
                xr::Binding::new(&haptic_left, path("/user/hand/left/output/haptic")?),
                xr::Binding::new(&haptic_right, path("/user/hand/right/output/haptic")?),
            ],
        );

        // KHR Simple Controller universal fallback
        instance.suggest_interaction_profile_bindings(
            path("/interaction_profiles/khr/simple_controller")?,
            &[
                xr::Binding::new(&left_grip_act, path("/user/hand/left/input/grip/pose")?),
                xr::Binding::new(&right_grip_act, path("/user/hand/right/input/grip/pose")?),
                xr::Binding::new(&select, path("/user/hand/right/input/select/click")?),
                xr::Binding::new(&menu, path("/user/hand/left/input/menu/click")?),
                xr::Binding::new(&aim, path("/user/hand/right/input/aim/pose")?),
                xr::Binding::new(&haptic_left, path("/user/hand/left/output/haptic")?),
                xr::Binding::new(&haptic_right, path("/user/hand/right/output/haptic")?),
            ],
        )?;

        session.attach_action_sets(&[&set])?;
        Ok(Self {
            set,
            rotate,
            zoom,
            select,
            trigger,
            left_squeeze,
            right_squeeze,
            trigger_down: false,
            left_grab_down: false,
            right_grab_down: false,
            aim: aim.create_space(session, xr::Path::NULL, xr::Posef::IDENTITY)?,
            left_grip: left_grip_act.create_space(session, xr::Path::NULL, xr::Posef::IDENTITY)?,
            right_grip: right_grip_act.create_space(
                session,
                xr::Path::NULL,
                xr::Posef::IDENTITY,
            )?,
            menu,
            haptic_left,
            haptic_right,
            selected: false,
            cursor: None,
        })
    }

    pub fn haptic_pulse(
        &self,
        session: &xr::Session<xr::Vulkan>,
        right_hand: bool,
        amplitude: f32,
        duration_ms: i64,
    ) {
        let action = if right_hand {
            &self.haptic_right
        } else {
            &self.haptic_left
        };
        let _ = action.apply_feedback(
            session,
            xr::Path::NULL,
            &xr::HapticVibration::new()
                .amplitude(amplitude.clamp(0.0, 1.0))
                .frequency(xr::FREQUENCY_UNSPECIFIED)
                .duration(xr::Duration::from_nanos(duration_ms.max(4) * 1_000_000)),
        );
    }

    pub fn sample(
        &mut self,
        session: &xr::Session<xr::Vulkan>,
        space: &xr::Space,
        time: xr::Time,
    ) -> Result<Hands> {
        session.sync_actions(&[(&self.set).into()])?;
        let rotate = self.rotate.state(session, xr::Path::NULL)?;
        let zoom = self.zoom.state(session, xr::Path::NULL)?;
        let select = self.select.state(session, xr::Path::NULL)?;
        let trigger = self.trigger.state(session, xr::Path::NULL)?;
        let l_sq = self.left_squeeze.state(session, xr::Path::NULL)?;
        let r_sq = self.right_squeeze.state(session, xr::Path::NULL)?;
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

        let down = (select.is_active && select.current_state)
            || (trigger.is_active
                && trigger.current_state >= if self.trigger_down { 0.30 } else { 0.65 });
        let click = down && !self.trigger_down;
        self.trigger_down = down;

        let left_grab = l_sq.is_active
            && l_sq.current_state >= if self.left_grab_down { 0.35 } else { 0.60 };
        let left_grab_start = left_grab && !self.left_grab_down;
        self.left_grab_down = left_grab;

        let right_grab = r_sq.is_active
            && r_sq.current_state >= if self.right_grab_down { 0.35 } else { 0.60 };
        let right_grab_start = right_grab && !self.right_grab_down;
        self.right_grab_down = right_grab;

        Ok(Hands {
            left: locate(&self.left_grip)?,
            right_grip: locate(&self.right_grip)?,
            right: locate(&self.aim)?,
            rotate: if rotate.is_active {
                glam::vec2(rotate.current_state.x, rotate.current_state.y)
            } else {
                glam::Vec2::ZERO
            },
            zoom: if zoom.is_active {
                zoom.current_state.y
            } else {
                0.0
            },
            down,
            click,
            left_grab,
            left_grab_start,
            right_grab,
            right_grab_start,
            toggle: menu.is_active && menu.changed_since_last_sync && menu.current_state,
        })
    }
}

#[derive(Default)]
pub struct Hands {
    pub left: Option<xr::Posef>,
    pub right_grip: Option<xr::Posef>,
    pub right: Option<xr::Posef>,
    pub rotate: glam::Vec2,
    pub zoom: f32,
    pub down: bool,
    pub click: bool,
    pub left_grab: bool,
    pub left_grab_start: bool,
    pub right_grab: bool,
    pub right_grab_start: bool,
    pub toggle: bool,
}
