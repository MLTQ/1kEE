//! Bounded reads from the prepared, north-up, little-endian SWOT VGG grid.
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub(super) const WIDTH: usize = 21_600;
pub(super) const HEIGHT: usize = 10_800;
pub(super) const OVERVIEW: [usize; 2] = [4096, 2048];
const NODATA: i16 = i16::MIN;

/// Native pixel bounds; x may extend beyond the dateline and wraps on read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Region {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
}

impl Region {
    pub fn around(lat: f32, lon: f32, half: f32) -> Self {
        // Pad and snap to whole degrees so small pans reuse the same texture.
        // Match the local scene's latitude/longitude span (cosine is applied
        // by its projector), with enough margin for the oblique viewport.
        let margin = (half * 3.0).clamp(0.5, 40.0);
        let west = (lon - margin).floor();
        let east = (lon + margin).ceil();
        let north = (lat + margin).ceil().min(90.0);
        let south = (lat - margin).floor().max(-90.0);
        Self {
            x: ((west + 180.0) * 60.0).round() as i32,
            y: ((90.0 - north) * 60.0).round() as i32,
            width: ((east - west) * 60.0).round() as usize,
            height: ((north - south) * 60.0).round() as usize,
        }
    }

    pub fn bounds(self) -> [f32; 4] {
        [
            self.x as f32 / 60.0 - 180.0,
            90.0 - (self.y as f32 + self.height as f32) / 60.0,
            (self.x as f32 + self.width as f32) / 60.0 - 180.0,
            90.0 - self.y as f32 / 60.0,
        ]
    }
}

pub(super) fn color(value: i16) -> egui::Color32 {
    if value == NODATA {
        return egui::Color32::TRANSPARENT;
    }
    // Values are stored as Eotvos * 10. Keep a fixed, symmetric +/-20 E scale.
    let t = (value as f32 / 200.0).clamp(-1.0, 1.0);
    let neutral = [28.0, 34.0, 42.0];
    let end = if t < 0.0 {
        [42.0, 126.0, 210.0]
    } else {
        [232.0, 104.0, 62.0]
    };
    let rgb = std::array::from_fn::<_, 3, _>(|i| {
        (neutral[i] + (end[i] - neutral[i]) * t.abs()).round() as u8
    });
    egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

pub(super) fn read_image(root: &Path, region: Option<Region>) -> Result<egui::ColorImage, String> {
    let (name, size) = if region.is_some() {
        ("vgg_21600x10800.bil", [WIDTH, HEIGHT])
    } else {
        ("vgg_4096x2048.bil", OVERVIEW)
    };
    let path = root.join(name);
    let mut file = std::fs::File::open(&path).map_err(|e| {
        format!(
            "Cannot open {}: {e}. Prepare SWOT data first.",
            path.display()
        )
    })?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length != (size[0] * size[1] * 2) as u64 {
        return Err(format!("Invalid SWOT grid size: {}", path.display()));
    }
    match region {
        None => {
            let mut bytes = vec![0; size[0] * size[1] * 2];
            file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
            Ok(egui::ColorImage {
                size,
                pixels: bytes
                    .chunks_exact(2)
                    .map(|b| color(i16::from_le_bytes([b[0], b[1]])))
                    .collect(),
            })
        }
        Some(region) => read_region(&mut file, region),
    }
}

fn read_region(file: &mut (impl Read + Seek), region: Region) -> Result<egui::ColorImage, String> {
    if region.width == 0 || region.height == 0 || region.width > WIDTH || region.height > HEIGHT {
        return Err("Invalid SWOT window".into());
    }
    let size = [region.width.min(1024), region.height.min(1024)];
    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    let mut row = vec![0u8; WIDTH * 2];
    for out_y in 0..size[1] {
        let y = region.y + ((out_y * 2 + 1) * region.height / (size[1] * 2)) as i32;
        if !(0..HEIGHT as i32).contains(&y) {
            pixels.extend(std::iter::repeat_n(egui::Color32::TRANSPARENT, size[0]));
            continue;
        }
        file.seek(SeekFrom::Start((y as usize * WIDTH * 2) as u64))
            .and_then(|_| file.read_exact(&mut row))
            .map_err(|e| e.to_string())?;
        for out_x in 0..size[0] {
            let x = (region.x + ((out_x * 2 + 1) * region.width / (size[0] * 2)) as i32)
                .rem_euclid(WIDTH as i32) as usize;
            pixels.push(color(i16::from_le_bytes([row[x * 2], row[x * 2 + 1]])));
        }
    }
    Ok(egui::ColorImage { size, pixels })
}

#[cfg(test)]
#[path = "grid_tests.rs"]
mod tests;
