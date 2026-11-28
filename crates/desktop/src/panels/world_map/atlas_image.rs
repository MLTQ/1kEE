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
/// Tuned for a bright chalky rag-paper lowland base with deep lithographic
/// oxide-umber mountain spines and glacial-celadon to Prussian-teal seas.
pub(crate) fn elevation_color(height: f32) -> [f32; 3] {
    const RAMP: &[(f32, [f32; 3])] = &[
        (-11_000.0, [14.0, 38.0, 56.0]),
        (-5_000.0, [26.0, 68.0, 88.0]),
        (-1_500.0, [62.0, 118.0, 134.0]),
        (-200.0, [156.0, 198.0, 196.0]),
        (-1.0, [216.0, 228.0, 222.0]),
        (0.0, [242.0, 236.0, 226.0]),
        (180.0, [235.0, 226.0, 212.0]),
        (500.0, [216.0, 200.0, 174.0]),
        (900.0, [188.0, 146.0, 110.0]),
        (1_200.0, [168.0, 102.0, 74.0]),
        (2_000.0, [136.0, 58.0, 48.0]),
        (3_000.0, [106.0, 46.0, 48.0]),
        (4_200.0, [162.0, 128.0, 118.0]),
        (5_500.0, [236.0, 228.0, 214.0]),
        (9_000.0, [250.0, 246.0, 238.0]),
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
