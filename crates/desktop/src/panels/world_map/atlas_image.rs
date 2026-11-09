//! Printed relief from measured GEBCO heights; no synthetic geography.
use egui::Color32;

pub(super) struct HeightGrid {
    pub width: usize,
    pub height: usize,
    pub metres: Vec<f32>,
}

impl HeightGrid {
    pub fn image(&self) -> Option<egui::ColorImage> {
        if self.width < 3 || self.height < 3 || self.metres.len() != self.width * self.height {
            return None;
        }
        let mut pixels = Vec::with_capacity(self.metres.len());
        let dy = 20_015_000.0 / self.height as f32;
        for y in 0..self.height {
            let latitude = 90.0 - (y as f32 + 0.5) * 180.0 / self.height as f32;
            let dx = 40_030_000.0 / self.width as f32 * latitude.to_radians().cos().max(0.05);
            for x in 0..self.width {
                let h = self.metres[y * self.width + x];
                if !valid(h) {
                    pixels.push(Color32::TRANSPARENT);
                    continue;
                }
                let sample = |x: usize, y: usize| {
                    let v = self.metres[y * self.width + x];
                    if valid(v) { v } else { h }
                };
                // Longitude wraps, latitude clamps. Correct physical spacing keeps
                // high-latitude ranges from flattening in the equirectangular grid.
                let east = sample((x + 1) % self.width, y);
                let west = sample((x + self.width - 1) % self.width, y);
                let north = sample(x, y.saturating_sub(1));
                let south = sample(x, (y + 1).min(self.height - 1));
                let exaggeration = if h < 0.0 { 18.0 } else { 12.0 };
                let sx = (east - west) / (2.0 * dx) * exaggeration;
                let sy = (north - south) / (2.0 * dy) * exaggeration;
                // Northwest light, 45 degrees above the map. Flat paper stays
                // bright; steep shaded faces pick up a cool charcoal ink tone.
                let diffuse = ((0.5 * sx - 0.5 * sy + std::f32::consts::FRAC_1_SQRT_2)
                    / (1.0 + sx * sx + sy * sy).sqrt())
                .clamp(0.0, 1.0);
                let light = 0.40 + 0.85 * diffuse;
                let base = elevation_color(h);
                let shadow = [42.0, 48.0, 53.0];
                let channels = std::array::from_fn::<_, 3, _>(|i| {
                    (shadow[i] + (base[i] - shadow[i]) * light).clamp(0.0, 255.0) as u8
                });
                pixels.push(Color32::from_rgb(channels[0], channels[1], channels[2]));
            }
        }
        Some(egui::ColorImage {
            size: [self.width, self.height],
            pixels,
        })
    }
}

fn valid(height: f32) -> bool {
    height.is_finite() && (-11_000.0..=9_000.0).contains(&height)
}

/// Height/depth tints, deliberately not geological classifications.
fn elevation_color(height: f32) -> [f32; 3] {
    const RAMP: &[(f32, [f32; 3])] = &[
        (-11_000.0, [40.0, 76.0, 96.0]),
        (-5_000.0, [62.0, 105.0, 125.0]),
        (-1_500.0, [100.0, 144.0, 155.0]),
        (-200.0, [163.0, 193.0, 191.0]),
        (-1.0, [202.0, 216.0, 204.0]),
        (0.0, [229.0, 220.0, 197.0]),
        (200.0, [220.0, 215.0, 186.0]),
        (650.0, [190.0, 191.0, 151.0]),
        (1_200.0, [191.0, 171.0, 127.0]),
        (2_000.0, [172.0, 132.0, 104.0]),
        (3_000.0, [151.0, 107.0, 96.0]),
        (4_200.0, [192.0, 171.0, 152.0]),
        (5_500.0, [234.0, 226.0, 211.0]),
        (9_000.0, [247.0, 241.0, 227.0]),
    ];
    for pair in RAMP.windows(2) {
        let [(low, a), (high, b)] = [pair[0], pair[1]];
        if height <= high {
            let t = ((height - low) / (high - low)).clamp(0.0, 1.0);
            return std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        }
    }
    RAMP.last().unwrap().1
}

#[cfg(test)]
#[path = "atlas_image_tests.rs"]
mod tests;
