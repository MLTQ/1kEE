pub const GLOBE_PITCH_LIMIT_RAD: f32 = 1.53;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoPoint {
    pub lat: f32,
    pub lon: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct GlobeViewState {
    pub yaw: f32,
    pub pitch: f32,
    pub local_center: GeoPoint,
    pub local_yaw: f32,
    pub local_pitch: f32,
    pub local_layer_spread: f32,
    pub zoom: f32,
    /// Zoom level used inside local terrain mode ([4, 60]).
    /// Independent of `zoom` so each mode keeps its own level.
    pub local_zoom: f32,
    /// Explicit GLOBE / LOCAL mode switch (not derived from zoom).
    pub local_mode: bool,
    pub auto_spin: bool,
    /// Enable cinematic meander (smooth random-walk camera drift).
    pub meander_mode: bool,
    /// Meander velocity scale 0.05 – 1.0 (set by the speed slider in the UI).
    pub meander_speed: f32,

    // ── Momentum velocities ────────────────────────────────────────────────
    /// Globe rotation velocity (rad/s).
    pub vel_yaw: f32,
    pub vel_pitch: f32,
    /// Local-mode pan velocity (deg/s in lat/lon space).
    pub vel_local_lat: f32,
    pub vel_local_lon: f32,
    /// Local-mode camera-angle rotation velocity (rad/s).
    pub vel_local_yaw: f32,
    pub vel_local_pitch: f32,
    /// Seconds any movement key has been continuously held — drives the
    /// acceleration ramp.  Resets to 0 when all keys are released.
    pub key_hold_secs: f32,
}

impl GlobeViewState {
    pub fn from_focus(point: GeoPoint) -> Self {
        let mut state = Self {
            yaw: 0.0,
            pitch: 0.0,
            local_center: point,
            local_yaw: -0.65,
            local_pitch: 0.98,
            local_layer_spread: 1.0,
            zoom: 1.0,
            local_zoom: 25.0,
            local_mode: false,
            auto_spin: false,
            meander_mode: false,
            meander_speed: 0.30,
            vel_yaw: 0.0,
            vel_pitch: 0.0,
            vel_local_lat: 0.0,
            vel_local_lon: 0.0,
            vel_local_yaw: 0.0,
            vel_local_pitch: 0.0,
            key_hold_secs: 0.0,
        };
        state.focus_on(point);
        state
    }

    pub fn focus_on(&mut self, point: GeoPoint) {
        self.yaw = point.lon.to_radians() - std::f32::consts::FRAC_PI_2;
        self.pitch = point
            .lat
            .to_radians()
            .clamp(-GLOBE_PITCH_LIMIT_RAD, GLOBE_PITCH_LIMIT_RAD);
        self.local_center = point;
        self.reset_local_camera();
    }

    pub fn reset_local_camera(&mut self) {
        self.local_yaw = -0.65;
        self.local_pitch = 0.98;
    }

    /// Returns the lat/lon at the center of the current globe viewport.
    /// This is the inverse of `focus_on`: yaw = lon - π/2, pitch = lat.
    pub fn globe_center_latlon(&self) -> GeoPoint {
        let lat = self.pitch.to_degrees().clamp(
            -GLOBE_PITCH_LIMIT_RAD.to_degrees(),
            GLOBE_PITCH_LIMIT_RAD.to_degrees(),
        );
        let lon_rad = self.yaw + std::f32::consts::FRAC_PI_2;
        let lon_deg = lon_rad.to_degrees();
        let lon = ((lon_deg + 180.0).rem_euclid(360.0)) - 180.0;
        GeoPoint { lat, lon }
    }
}
