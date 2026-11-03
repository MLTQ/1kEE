//! Read one bounded regional, physical-mGal raster without loading on the UI thread.
use serde::Deserialize;
use std::io::Read;
use std::path::Path;

#[derive(Deserialize)]
pub(super) struct Metadata {
    pub schema: u32,
    pub id: String,
    pub units: String,
    pub size: [usize; 2],
    pub bounds: [f32; 4],
    pub nodata: f32,
}

pub(super) struct Image {
    pub image: egui::ColorImage,
    pub bounds: [f32; 4],
}

pub(super) fn color(value: f32, range: f32) -> egui::Color32 {
    if !value.is_finite() || value == -999999.0 {
        return egui::Color32::TRANSPARENT;
    }
    let t = (value / range).clamp(-1.0, 1.0);
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

pub(super) fn read(root: &Path, id: &str, limit: usize, range: f32) -> Result<Image, String> {
    let folder = root.join("gravity/ncei").join(id);
    let manifest = std::fs::read(folder.join("manifest.json"))
        .map_err(|e| format!("Gravity data unavailable on the selected drive: {e}"))?;
    let meta: Metadata = serde_json::from_slice(&manifest).map_err(|e| e.to_string())?;
    let [w, h] = meta.size;
    let [west, south, east, north] = meta.bounds;
    if meta.schema != 1
        || meta.id != id
        || meta.units != "mGal"
        || meta.nodata != -999999.0
        || w == 0
        || h == 0
        || w > 8192
        || h > 8192
        || !meta.bounds.iter().all(|v| v.is_finite())
        || east <= west
        || east - west > 360.0
        || north <= south
        || south < -90.0
        || north > 90.0
        || !range.is_finite()
        || range <= 0.0
        || limit == 0
        || limit > 4096
    {
        return Err("Invalid prepared gravity map metadata".into());
    }
    let mut file = std::fs::File::open(folder.join("grid.bil")).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() != (w * h * 4) as u64 {
        return Err("Incomplete gravity map".into());
    }
    // Stream one row at a time. Preserve every prepared pixel when it fits.
    let size = [w.min(limit), h.min(limit)];
    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    let mut row = vec![0; w * 4];
    let mut source_y = 0;
    for y in 0..size[1] {
        let wanted = (2 * y + 1) * h / (2 * size[1]);
        while source_y <= wanted {
            file.read_exact(&mut row).map_err(|e| e.to_string())?;
            source_y += 1;
        }
        for x in 0..size[0] {
            let offset = (2 * x + 1) * w / (2 * size[0]) * 4;
            pixels.push(color(
                f32::from_le_bytes(row[offset..offset + 4].try_into().unwrap()),
                range,
            ));
        }
    }
    Ok(Image {
        image: egui::ColorImage { size, pixels },
        bounds: meta.bounds,
    })
}

#[cfg(test)]
#[path = "grid_tests.rs"]
mod tests;
