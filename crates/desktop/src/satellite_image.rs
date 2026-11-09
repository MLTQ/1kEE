//! Prepare a display-size, north-up WGS84 texture from a projected satellite image.
use crate::panels::world_map::srtm_focus_cache::gdal;
use serde::Deserialize;
use std::{path::Path, process::Command, time::Duration};

pub(crate) struct SatelliteImage {
    pub bounds: [f32; 4],
    pub pixels: egui::ColorImage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RasterInfo {
    size: [u32; 2],
    coordinate_system: Option<CoordinateSystem>,
    geo_transform: Option<[f64; 6]>,
    bands: Vec<Band>,
}

#[derive(Deserialize)]
struct CoordinateSystem {
    wkt: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Band {
    #[serde(rename = "type")]
    sample_type: String,
    color_interpretation: String,
}

fn info(path: &Path, driver: &str) -> Result<RasterInfo, String> {
    let output = Command::new(gdal::gdal_tool_path("gdalinfo"))
        .args(["-json", "-nomd", "-norat", "-noct", "-if", driver])
        .arg(path)
        .output()
        .map_err(|e| {
            format!("Cannot run gdalinfo: {e}. Set the GDAL folder in Settings → Files & Cache.")
        })?;
    if !output.status.success() {
        return Err(format!(
            "Cannot inspect raster: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("Cannot read raster metadata: {e}"))
}

fn validate_source(source: &RasterInfo) -> Result<(), String> {
    if source
        .coordinate_system
        .as_ref()
        .is_none_or(|s| s.wkt.is_empty())
        || source.geo_transform.is_none()
    {
        return Err("The image needs a coordinate system and pixel transform. Export the projection as GeoTIFF in SatDump.".into());
    }
    let colors: Vec<_> = source
        .bands
        .iter()
        .map(|b| b.color_interpretation.as_str())
        .collect();
    if !matches!(
        colors.as_slice(),
        ["Gray"] | ["Gray", "Alpha"] | ["Red", "Green", "Blue"] | ["Red", "Green", "Blue", "Alpha"]
    ) || source
        .bands
        .iter()
        .any(|b| !matches!(b.sample_type.as_str(), "Byte" | "UInt16"))
    {
        return Err(
            "Export an 8- or 16-bit grayscale or RGB display image with optional alpha.".into(),
        );
    }
    Ok(())
}

fn preview_grid(grid: &RasterInfo, max_side: usize) -> Result<([f32; 4], [u32; 2]), String> {
    let [w, h] = grid.size;
    let [west, dx, rx, north, ry, dy] =
        grid.geo_transform.ok_or("Missing WGS84 pixel transform")?;
    let east = west + dx * f64::from(w);
    let south = north + dy * f64::from(h);
    if w == 0
        || h == 0
        || rx != 0.0
        || ry != 0.0
        || dx <= 0.0
        || dy >= 0.0
        || ![west, south, east, north].iter().all(|v| v.is_finite())
        || north > 90.0 - dy
        || south < -90.0 + dy
        || east - west > 360.0 + dx
    {
        return Err("Invalid WGS84 image extent.".into());
    }
    // GDAL can round a suggested output edge by a fraction of one pixel.
    let bounds = [
        west,
        south.max(-90.0),
        east.min(west + 360.0),
        north.min(90.0),
    ]
    .map(|v| v as f32);
    if bounds[2] <= bounds[0] || bounds[3] <= bounds[1] {
        return Err("Empty WGS84 image extent.".into());
    }
    let scale = (max_side.clamp(1, 4096) as f64 / f64::from(w.max(h))).min(1.0);
    Ok((
        bounds,
        [w, h].map(|n| (f64::from(n) * scale).round().max(1.0) as u32),
    ))
}

pub(crate) fn read(path: &Path, max_side: usize) -> Result<SatelliteImage, String> {
    validate_source(&info(path, "GTiff")?)?;
    let dir = tempfile::tempdir().map_err(|e| format!("Cannot prepare raster: {e}"))?;
    let warped = dir.path().join("geographic.vrt");
    let png = dir.path().join("display.png");
    let warp_command = |format: &str| {
        let mut cmd = Command::new(gdal::gdal_tool_path("gdalwarp"));
        cmd.args([
            "-q",
            "-if",
            "GTiff",
            "-of",
            format,
            "-t_srs",
            "EPSG:4326",
            "-dstalpha",
        ]);
        cmd
    };
    // Ask GDAL for the footprint without materializing a full-resolution image.
    let mut warp = warp_command("VRT");
    warp.arg(path).arg(&warped);
    gdal::run_command_with_timeout(warp, "Satellite reprojection", Duration::from_secs(90))
        .map_err(|e| format!("Cannot project GeoTIFF: {e}"))?;
    let (bounds, [w, h]) = preview_grid(&info(&warped, "VRT")?, max_side)?;
    let mut preview = warp_command("PNG");
    preview
        .arg("-te")
        .args(bounds.map(|edge| edge.to_string()))
        .args(["-ts", &w.to_string(), &h.to_string()])
        .arg(path)
        .arg(&png);
    gdal::run_command_with_timeout(preview, "Satellite preview", Duration::from_secs(120))
        .map_err(|e| format!("Cannot prepare GeoTIFF preview: {e}"))?;
    let image = image::open(png)
        .map_err(|e| format!("Cannot decode satellite preview: {e}"))?
        .into_rgba8();
    Ok(SatelliteImage {
        bounds,
        pixels: egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.as_raw()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires GDAL command-line tools"]
    fn import_reprojects_and_preserves_visible_black_and_transparency() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("capture.png");
        let tiff = dir.path().join("pass.TIFF");
        let image = image::RgbaImage::from_fn(8, 4, |x, y| {
            image::Rgba(match (x < 4, y < 2) {
                (true, true) => [0, 0, 0, 255],
                (false, true) => [255, 0, 0, 255],
                (true, false) => [0, 255, 0, 255],
                (false, false) => [0, 0, 0, 0],
            })
        });
        image.save(&input).unwrap();
        // Centered at the equator in Mercator. Geographic edges are ±2° x ±1°.
        assert!(
            Command::new(gdal::gdal_tool_path("gdal_translate"))
                .args([
                    "-q",
                    "-a_srs",
                    "EPSG:3857",
                    "-a_ullr",
                    "-222639",
                    "111325",
                    "222639",
                    "-111325"
                ])
                .arg(&input)
                .arg(&tiff)
                .status()
                .unwrap()
                .success()
        );
        let result = read(&tiff, 4).unwrap();
        for (edge, expected) in result.bounds.into_iter().zip([-2.0, -1.0, 2.0, 1.0]) {
            assert!((edge - expected).abs() < 0.001, "{edge} vs {expected}");
        }
        assert_eq!(result.pixels.size, [4, 2]);
        let p = result.pixels.pixels;
        assert_eq!(
            [p[0], p[3], p[4], p[7]],
            [
                egui::Color32::BLACK,
                egui::Color32::RED,
                egui::Color32::GREEN,
                egui::Color32::TRANSPARENT
            ]
        );
        let raw = dir.path().join("raw.tif");
        image.save(&raw).unwrap();
        assert!(read(&raw, 4).err().unwrap().contains("coordinate system"));
    }
}
