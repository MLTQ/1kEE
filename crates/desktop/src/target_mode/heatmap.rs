//! Local height limits and the sRGB colors shared by the Target shader and legend.
pub(crate) const COLOR_MODE: u32 = 4;
pub(crate) const COLORS: [[u8; 3]; 7] = [
    [38, 85, 222],
    [19, 176, 243],
    [37, 211, 179],
    [100, 220, 84],
    [250, 225, 59],
    [255, 139, 32],
    [200, 54, 25],
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeightRange {
    pub low: f32,
    pub high: f32,
}

impl HeightRange {
    pub const EMPTY: Self = Self {
        low: f32::INFINITY,
        high: f32::NEG_INFINITY,
    };

    pub fn include(&mut self, height: f32) {
        if height.is_finite() {
            self.low = self.low.min(height);
            self.high = self.high.max(height);
        }
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            low: self.low.min(other.low),
            high: self.high.max(other.high),
        }
    }
}
