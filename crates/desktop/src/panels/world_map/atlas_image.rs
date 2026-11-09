//! Printed relief from measured GEBCO heights; no synthetic geography.
use egui::Color32;

pub(super) struct HeightGrid {
    pub width: usize,
    pub height: usize,
    pub metres: Vec<f32>,
}

impl HeightGrid {
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
        let dy = (bounds[3] - bounds[1]) * 111_195.0 / self.height as f32;
        let wrap = bounds[2] - bounds[0] >= 359.99;
        let mut pixels = vec![Color32::TRANSPARENT; self.metres.len()];
        pixels
            .par_chunks_mut(self.width)
            .enumerate()
            .for_each(|(y, row)| {
                let latitude =
                    bounds[3] - (y as f32 + 0.5) * (bounds[3] - bounds[1]) / self.height as f32;
                let dx = (bounds[2] - bounds[0]) * 111_195.0 / self.width as f32
                    * latitude.to_radians().cos().abs().max(0.05);
                let exaggeration = (dy / 350.0).sqrt().clamp(1.0, 4.0) * 2.6;
                for (x, pixel) in row.iter_mut().enumerate() {
                    let h = self.metres[y * self.width + x];
                    if !valid(h) {
                        continue;
                    }
                    let sample = |ox: isize, oy: isize| {
                        let sx = if wrap {
                            (x as isize + ox).rem_euclid(self.width as isize) as usize
                        } else {
                            (x as isize + ox).clamp(0, self.width as isize - 1) as usize
                        };
                        let sy = (y as isize + oy).clamp(0, self.height as isize - 1) as usize;
                        let v = self.metres[sy * self.width + sx];
                        if valid(v) { v } else { h }
                    };
                    // Fine ridge facets plus a broad normal keep the large ranges
                    // legible without reducing the source to a blurred height map.
                    let slope = |radius: isize| {
                        let n = radius as f32;
                        (
                            (sample(radius, 0) - sample(-radius, 0)) / (2.0 * n * dx)
                                * exaggeration,
                            (sample(0, -radius) - sample(0, radius)) / (2.0 * n * dy)
                                * exaggeration,
                        )
                    };
                    let (sx, sy) = slope(1);
                    let (bx, by) = slope(6);
                    let light = |x: f32, y: f32| {
                        ((0.57 * x - 0.57 * y + 0.59) / (1.0 + x * x + y * y).sqrt())
                            .clamp(0.0, 1.0)
                    };
                    let diffuse = light(sx, sy) * 0.78 + light(bx, by) * 0.22;
                    let mut horizon = 0.0f32;
                    let mut enclosure = 0.0f32;
                    // Measured terrain casts soft northwest shadows; valley
                    // enclosure controls skylight. No noise or invented ridges.
                    for radius in [2isize, 6, 18] {
                        let distance = radius as f32 * (dx * dx + dy * dy).sqrt();
                        horizon =
                            horizon.max((sample(-radius, -radius) - h) * exaggeration / distance);
                        let high = (sample(radius, 0)
                            + sample(-radius, 0)
                            + sample(0, radius)
                            + sample(0, -radius))
                            * 0.25
                            - h;
                        enclosure +=
                            (high * exaggeration / (radius as f32 * (dx + dy) * 0.5)).max(0.0);
                    }
                    let cast = 1.0 - ((horizon - 0.45) / 0.65).clamp(0.0, 1.0) * 0.70;
                    let ambient = 1.0 - (enclosure * 0.12).clamp(0.0, 0.35);
                    let intensity = (0.30 + 1.18 * diffuse * cast) * ambient;
                    let base = elevation_color(h);
                    let ink = if h < 0.0 {
                        [22.0, 35.0, 46.0]
                    } else {
                        [34.0, 29.0, 31.0]
                    };
                    let channels = std::array::from_fn::<_, 3, _>(|i| {
                        (ink[i] + (base[i] - ink[i]) * intensity).clamp(0.0, 255.0) as u8
                    });
                    *pixel = Color32::from_rgb(channels[0], channels[1], channels[2]);
                }
            });
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
        (1_200.0, [187.0, 160.0, 118.0]),
        (2_000.0, [164.0, 119.0, 94.0]),
        (3_000.0, [146.0, 96.0, 91.0]),
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
