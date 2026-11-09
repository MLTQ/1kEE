//! Saved Atlas art direction; changing light never changes the terrain source.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightMode {
    Sun,
    #[default]
    #[serde(other)]
    Studio,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AtlasStyle {
    pub light_mode: LightMode,
    pub azimuth_deg: f32,
    pub altitude_deg: f32,
    pub time_offset_hours: f32,
    pub color_richness: f32,
    pub relief_strength: f32,
}
impl Default for AtlasStyle {
    fn default() -> Self {
        Self {
            light_mode: LightMode::Studio,
            azimuth_deg: 315.0,
            altitude_deg: 35.0,
            time_offset_hours: 0.0,
            color_richness: 1.0,
            relief_strength: 1.0,
        }
    }
}
impl AtlasStyle {
    pub fn normalized(mut self) -> Self {
        let default = Self::default();
        let finite = |v: f32, fallback| if v.is_finite() { v } else { fallback };
        self.azimuth_deg = finite(self.azimuth_deg, default.azimuth_deg).rem_euclid(360.0);
        self.altitude_deg = finite(self.altitude_deg, default.altitude_deg).clamp(5.0, 85.0);
        self.time_offset_hours = finite(self.time_offset_hours, 0.0).clamp(-12.0, 12.0);
        self.color_richness = finite(self.color_richness, 1.0).clamp(0.0, 1.8);
        self.relief_strength = finite(self.relief_strength, 1.0).clamp(0.0, 2.0);
        self
    }

    /// Studio: east/north/up light vector. Sun: Earth-fixed XYZ vector, mode=1.
    /// Minute quantization avoids re-rendering millions of triangles every tick.
    pub fn light(self, jd: f64) -> [f32; 4] {
        let style = self.normalized();
        if style.light_mode == LightMode::Sun {
            let jd = if jd.is_finite() {
                jd
            } else {
                crate::stellar_time::JD_J2000
            };
            let jd = ((jd + style.time_offset_hours as f64 / 24.0) * 1440.0).floor() / 1440.0;
            let (ra, dec) =
                crate::planet_ephemeris::geocentric_radec(crate::planet_ephemeris::Planet::Sun, jd)
                    .unwrap_or((0.0, 0.0));
            let lon = (ra - crate::stellar_time::gmst_deg(jd)).to_radians();
            let lat = dec.to_radians();
            return [
                (lat.cos() * lon.cos()) as f32,
                lat.sin() as f32,
                (lat.cos() * lon.sin()) as f32,
                1.0,
            ];
        }
        let az = style.azimuth_deg.to_radians();
        let alt = style.altitude_deg.to_radians();
        [az.sin() * alt.cos(), az.cos() * alt.cos(), alt.sin(), 0.0]
    }
}

#[cfg(test)]
#[path = "atlas_style_tests.rs"]
mod tests;
