#[path = "polar.rs"]
pub(crate) mod polar;

pub const LOCAL_MAX_TILT: f32 = 75.0_f32.to_radians();
pub const MAX_CAMERA_TILT_RAD: f32 = 78.0_f32.to_radians();
pub const GLOBE_PITCH_LIMIT_RAD: f32 = 1.53;

/// Closest continuous camera distance to ground target (metres).
/// At 20 m range with 38° vertical FOV, viewport spans ~14 m across — building/tree/LiDAR detail.
pub const MIN_CAMERA_RANGE_M: f64 = 20.0;
/// Widest continuous camera distance expressed as a multiple of planetary radius.
pub const MAX_CAMERA_RANGE_FACTOR: f64 = 3.8;
/// Default orbital camera range on Earth (~14,500 km, framing the full planet).
pub const DEFAULT_EARTH_RANGE_M: f64 = 14_500_000.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectionMode {
    #[default]
    Perspective,
    Orthographic,
}

impl ProjectionMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Perspective => "PERSP",
            Self::Orthographic => "ORTHO",
        }
    }
    pub const fn is_ortho(self) -> bool {
        matches!(self, Self::Orthographic)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoPoint {
    pub lat: f32,
    pub lon: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoPoint64 {
    pub lat: f64,
    pub lon: f64,
}

impl GeoPoint64 {
    pub const fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }
    pub fn to_f32(self) -> GeoPoint {
        GeoPoint {
            lat: self.lat as f32,
            lon: self.lon as f32,
        }
    }
}

impl From<GeoPoint> for GeoPoint64 {
    fn from(p: GeoPoint) -> Self {
        Self {
            lat: p.lat as f64,
            lon: p.lon as f64,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GlobeViewState {
    // ── Unified continuous 3D camera state (planet scale → metre scale) ───
    pub target_lat: f64,
    pub target_lon: f64,
    /// Ground elevation (metres above mean sphere) under the camera target,
    /// so close-range zooming over mountains/plateaus orbits above the surface.
    pub target_elev_m: f64,
    /// Distance from camera eye to ground target in metres (logarithmic zoom).
    pub range_m: f64,
    /// Azimuth rotation around target radial up (radians; 0 = North-up).
    pub heading: f32,
    /// Oblique tilt from nadir (radians; 0 = straight down, MAX_CAMERA_TILT_RAD = horizon).
    pub tilt: f32,
    /// Perspective or Orthographic projection on the same unified 3D camera.
    pub projection: ProjectionMode,

    // ── Legacy compatibility fields (removed in M3 once scenes merge) ──────
    pub yaw: f32,
    pub pitch: f32,
    pub local_center: GeoPoint,
    pub local_yaw: f32,
    pub local_pitch: f32,
    pub local_layer_spread: f32,
    pub zoom: f32,
    /// Zoom level used inside local terrain mode ([1, 60]).
    pub local_zoom: f32,
    /// Explicit GLOBE / LOCAL mode switch (being retired in M3).
    pub local_mode: bool,
    /// The VR table consumes a rectangular longitude/latitude texture.
    pub(crate) geographic_capture: bool,
    pub auto_spin: bool,
    /// Enable cinematic meander (smooth random-walk camera drift).
    pub meander_mode: bool,
    /// Meander velocity scale 0.05 – 1.0 (set by the speed slider in the UI).
    pub meander_speed: f32,

    // ── Unified continuous 3D camera velocities ──────────────────────────
    /// Ground-plane coast velocity in metres/sec along screen right & up.
    pub vel_pan_right_m: f64,
    pub vel_pan_up_m: f64,
    /// Orbit velocity (rad/s) for heading and tilt.
    pub vel_heading: f32,
    pub vel_tilt: f32,

    // ── Momentum velocities ────────────────────────────────────────────────
    /// Globe rotation velocity (rad/s).
    pub vel_yaw: f32,
    pub vel_pitch: f32,
    /// Local pan velocity in chart degrees/s (north/east arc in a polar chart).
    pub vel_local_lat: f32,
    pub vel_local_lon: f32,
    /// Camera-angle rotation velocity (rad/s).
    pub vel_local_yaw: f32,
    pub vel_local_pitch: f32,
    /// Seconds any movement key has been continuously held — drives the
    /// acceleration ramp.  Resets to 0 when all keys are released.
    pub key_hold_secs: f32,
}

impl GlobeViewState {
    pub fn from_focus(point: GeoPoint) -> Self {
        let mut state = Self {
            target_lat: point.lat as f64,
            target_lon: point.lon as f64,
            target_elev_m: 0.0,
            range_m: DEFAULT_EARTH_RANGE_M,
            heading: 0.0,
            tilt: 0.0,
            projection: ProjectionMode::Perspective,
            vel_pan_right_m: 0.0,
            vel_pan_up_m: 0.0,
            vel_heading: 0.0,
            vel_tilt: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            local_center: point,
            local_yaw: -0.65,
            local_pitch: 0.98,
            local_layer_spread: 1.0,
            zoom: 1.0,
            local_zoom: 25.0,
            local_mode: false,
            geographic_capture: false,
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
        self.target_lat = (point.lat as f64).clamp(-90.0, 90.0);
        self.target_lon = normalize_lon_f64(point.lon as f64);
        self.yaw = point.lon.to_radians() - std::f32::consts::FRAC_PI_2;
        self.pitch = point
            .lat
            .to_radians()
            .clamp(-GLOBE_PITCH_LIMIT_RAD, GLOBE_PITCH_LIMIT_RAD);
        self.local_center = point;
        self.reset_local_camera();
    }

    pub fn focus_on_f64(&mut self, point: GeoPoint64) {
        self.target_lat = point.lat.clamp(-90.0, 90.0);
        self.target_lon = normalize_lon_f64(point.lon);
        self.sync_legacy_from_unified();
    }

    pub fn target_geo(&self) -> GeoPoint64 {
        GeoPoint64 {
            lat: self.target_lat,
            lon: self.target_lon,
        }
    }

    /// Maximum allowed tilt angle for the current camera distance: full oblique
    /// tilt close to the surface, smoothly easing toward nadir at orbital scale
    /// so the user doesn't tilt off into empty space.
    pub fn max_tilt_for_range(range_m: f64, body_radius_m: f64) -> f32 {
        let ratio = (range_m / body_radius_m.max(1.0)) as f32;
        // Full 78° tilt below 0.35 R; smoothly ramps down to 0° by 1.6 R.
        let t = ((ratio - 0.35) / (1.60 - 0.35)).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        MAX_CAMERA_TILT_RAD * (1.0 - smooth)
    }

    pub fn max_range_for_body(body_radius_m: f64) -> f64 {
        body_radius_m * MAX_CAMERA_RANGE_FACTOR
    }

    /// Approximate equivalent `range_m` for a legacy Globe zoom value (`[0.6, 50.0]`).
    pub fn range_for_legacy_globe_zoom(zoom: f32, body_radius_m: f64) -> f64 {
        let zoom_t = ((zoom.clamp(0.6, 50.0).ln() - 0.6_f32.ln())
            / (50.0_f32.ln() - 0.6_f32.ln()))
            .clamp(0.0, 1.0) as f64;
        // Maps zoom=0.6 -> ~3.1*R (full planet), zoom=1.0 -> ~2.28*R, zoom=50 -> ~0.22*R (~1400 km)
        let log_max = (body_radius_m * 3.1).ln();
        let log_min = (body_radius_m * 0.22).ln();
        (log_max + (log_min - log_max) * zoom_t).exp()
    }

    /// Approximate equivalent `range_m` for a legacy Local zoom value (`[1.0, 60.0]`).
    pub fn range_for_legacy_local_zoom(local_zoom: f32) -> f64 {
        // visual_half_extent runs 4.50 deg (~500 km) at z=1 to 0.005 deg (~556 m) at z=60.
        // With 38 deg vertical FOV (tan(19 deg) ≈ 0.3443), half_h_m = range_m * 0.3443.
        let half_deg = crate::panels::world_map::vr_local_extent(local_zoom) as f64;
        let half_m = half_deg * 111_320.0;
        (half_m / (19.0_f64.to_radians().tan())).clamp(MIN_CAMERA_RANGE_M, 25_000_000.0)
    }

    /// Keep legacy globe/local fields coherent with the unified continuous camera.
    pub fn sync_legacy_from_unified(&mut self) {
        self.local_center = GeoPoint {
            lat: self.target_lat as f32,
            lon: self.target_lon as f32,
        };
        self.yaw = (self.target_lon as f32).to_radians() - std::f32::consts::FRAC_PI_2;
        self.pitch = (self.target_lat as f32)
            .to_radians()
            .clamp(-GLOBE_PITCH_LIMIT_RAD, GLOBE_PITCH_LIMIT_RAD);
        self.local_yaw = self.heading;
        self.local_pitch = self.tilt.clamp(0.0, LOCAL_MAX_TILT);
        // Derive approximate legacy zoom scalars for any code still reading them.
        let half_m = self.range_m * 19.0_f64.to_radians().tan();
        let half_deg = (half_m / 111_320.0) as f32;
        self.local_zoom = if half_deg >= 4.5 {
            1.0
        } else if half_deg <= 0.005 {
            60.0
        } else {
            // Smooth monotonic inverse approximation over [4.5 -> 1.0, 0.005 -> 60.0]
            let t = ((4.5_f32.ln() - half_deg.ln()) / (4.5_f32.ln() - 0.005_f32.ln())).clamp(0.0, 1.0);
            (1.0_f32.ln() + t * (60.0_f32.ln() - 1.0_f32.ln())).exp()
        };
        let ratio = (self.range_m / 6_371_008.8).clamp(0.22, 3.1);
        let zt = ((3.1_f64.ln() - ratio.ln()) / (3.1_f64.ln() - 0.22_f64.ln())).clamp(0.0, 1.0);
        self.zoom = (0.6_f64.ln() + zt * (50.0_f64.ln() - 0.6_f64.ln())).exp() as f32;
    }

    /// Cancel every momentum source when an idle controller takes/releases control.
    pub fn stop_motion(&mut self) {
        self.auto_spin = false;
        self.meander_mode = false;
        self.vel_pan_right_m = 0.0;
        self.vel_pan_up_m = 0.0;
        self.vel_heading = 0.0;
        self.vel_tilt = 0.0;
        self.vel_yaw = 0.0;
        self.vel_pitch = 0.0;
        self.vel_local_lat = 0.0;
        self.vel_local_lon = 0.0;
        self.vel_local_yaw = 0.0;
        self.vel_local_pitch = 0.0;
        self.key_hold_secs = 0.0;
    }

    pub fn reset_local_camera(&mut self) {
        if polar::pole_for(self.local_center.lat) != 0.0 {
            self.look_down();
        } else {
            self.local_yaw = -0.65;
            self.local_pitch = 0.98;
        }
    }

    pub fn look_down(&mut self) {
        self.heading = 0.0;
        self.tilt = 0.0;
        self.vel_heading = 0.0;
        self.vel_tilt = 0.0;
        self.local_yaw = 0.0;
        self.local_pitch = 0.0;
        self.vel_local_pitch = 0.0;
        self.vel_local_yaw = 0.0;
    }

    /// Returns the lat/lon at the center of the current viewport.
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

pub fn normalize_lon_f64(lon: f64) -> f64 {
    if !lon.is_finite() {
        return 0.0;
    }
    ((lon + 180.0).rem_euclid(360.0)) - 180.0
}
