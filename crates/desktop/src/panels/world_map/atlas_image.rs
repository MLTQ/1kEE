//! Unlit elevation/depth pigments; lighting is independent of these colors.
use egui::Color32;

pub(super) struct HeightGrid {
    pub width: usize,
    pub height: usize,
    pub metres: Vec<f32>,
}

impl HeightGrid {
    #[cfg(test)]
    pub fn image(&self) -> Option<egui::ColorImage> {
        self.image_for([-180.0, -90.0, 180.0, 90.0])
    }

    pub fn image_for(&self, bounds: [f32; 4]) -> Option<egui::ColorImage> {
        if self.width < 3
            || self.height < 3
            || self.metres.len() != self.width * self.height
            || !bounds.iter().all(|v| v.is_finite())
            || bounds[0] >= bounds[2]
            || bounds[1] >= bounds[3]
        {
            return None;
        }
        use rayon::prelude::*;
        // Store pigments only. All illumination comes from measured heights at
        // render time, so moving the light cannot leave a baked shadow behind.
        let pixels = self
            .metres
            .par_iter()
            .map(|&h| {
                if !valid(h) {
                    return Color32::TRANSPARENT;
                }
                let c = elevation_color(h);
                Color32::from_rgb(c[0] as u8, c[1] as u8, c[2] as u8)
            })
            .collect();
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
        (-11_000.0, [16.0, 43.0, 65.0]),
        (-5_000.0, [28.0, 72.0, 96.0]),
        (-1_500.0, [65.0, 121.0, 144.0]),
        (-200.0, [145.0, 190.0, 191.0]),
        (-1.0, [202.0, 218.0, 207.0]),
        (0.0, [237.0, 227.0, 209.0]),
        (200.0, [229.0, 214.0, 185.0]),
        (650.0, [211.0, 178.0, 135.0]),
        (1_200.0, [198.0, 126.0, 89.0]),
        (2_000.0, [183.0, 78.0, 61.0]),
        (3_000.0, [158.0, 58.0, 60.0]),
        (4_200.0, [193.0, 141.0, 126.0]),
        (5_500.0, [234.0, 220.0, 202.0]),
        (9_000.0, [248.0, 241.0, 226.0]),
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
