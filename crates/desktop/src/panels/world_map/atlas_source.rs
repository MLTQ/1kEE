//! Bounded, offline GDAL windows from original elevation files.
use super::{atlas_image::HeightGrid, atlas_viewport::Region, srtm_focus_cache::gdal};
use crate::{model::GeoPoint, terrain_assets, threedep};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

pub(super) struct Detail {
    pub grid: HeightGrid,
    pub region: Region,
    pub source: &'static str,
}

pub(super) fn load(root: Option<&Path>, region: Region, max_edge: usize) -> Option<Detail> {
    let data = terrain_assets::find_data_root(root)?;
    let base = gdal::find_gebco_topography_tiles(&data);
    if base.is_empty() {
        return None;
    }
    let span = region.span();
    let bounds = region.bounds();
    let srtm = if span[0] <= 8.0 && span[1] <= 8.0 {
        terrain_assets::find_srtm_root(root)
            .map(|root| srtm_files(&root, bounds))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let fine = if span[0] <= 0.15 && span[1] <= 0.15 {
        terrain_assets::find_derived_root(root)
            .map(|root| fine_files(&root, bounds))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let (spacing, source) = if !fine.is_empty() {
        (1.0 / 30.0, "cached fine elevation")
    } else if !srtm.is_empty() {
        (1.0, "SRTM · 1 arcsecond land")
    } else {
        (15.0, "GEBCO · 15 arcsecond terrain")
    };
    let size = region.size(spacing, max_edge);
    let scratch = tempfile::tempdir().ok()?;
    let metres = warp(&base, bounds, size, scratch.path(), "ocean")?;
    let mut grid = HeightGrid {
        width: size[0],
        height: size[1],
        metres,
    };
    for (name, files) in [("land", srtm), ("fine", fine)] {
        if !files.is_empty()
            && let Some(land) = warp(&files, bounds, size, scratch.path(), name)
        {
            merge_land(&mut grid.metres, &land);
        }
    }
    Some(Detail {
        grid,
        region,
        source,
    })
}

fn merge_land(base: &mut [f32], land: &[f32]) {
    for (base, &land) in base.iter_mut().zip(land) {
        // SRTM sea pixels are often zero; they must not erase measured depths.
        if land.is_finite() && (-11000.0..=9000.0).contains(&land) && (land > 0.0 || *base >= 0.0) {
            *base = land;
        }
    }
}

fn srtm_files(root: &Path, b: [f32; 4]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for lat in (b[1].floor() as i32).max(-90)..(b[3].ceil() as i32).min(90) {
        for unwrapped in b[0].floor() as i32..b[2].ceil() as i32 {
            let lon = (unwrapped + 180).rem_euclid(360) - 180;
            let path = root.join(format!(
                "{}{:02}{}{:03}.tif",
                if lat >= 0 { 'N' } else { 'S' },
                lat.abs(),
                if lon >= 0 { 'E' } else { 'W' },
                lon.abs()
            ));
            if path.exists() {
                paths.push(path);
            }
        }
    }
    paths
}

fn fine_files(root: &Path, b: [f32; 4]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for y in (b[1] / threedep::CHUNK_DEG).floor() as i32..(b[3] / threedep::CHUNK_DEG).ceil() as i32
    {
        for x in
            (b[0] / threedep::CHUNK_DEG).floor() as i32..(b[2] / threedep::CHUNK_DEG).ceil() as i32
        {
            let point = GeoPoint {
                lat: (y as f32 + 0.5) * threedep::CHUNK_DEG,
                lon: ((x as f32 + 0.5) * threedep::CHUNK_DEG + 180.0).rem_euclid(360.0) - 180.0,
            };
            if let Some(path) = threedep::peek_chunk(root, threedep::chunk_key_for(point)) {
                paths.push(path);
            }
        }
    }
    paths
}

/// Split output columns at longitude zones. Pixel edges/centres stay on one
/// exact grid, even when the half-pixel edge straddles the date line.
fn warp(
    files: &[PathBuf],
    b: [f32; 4],
    size: [usize; 2],
    scratch: &Path,
    name: &str,
) -> Option<Vec<f32>> {
    let mut out = vec![f32::NAN; size[0] * size[1]];
    let dx = (b[2] as f64 - b[0] as f64) / size[0] as f64;
    let mut start = 0;
    while start < size[0] {
        let center = b[0] as f64 + (start as f64 + 0.5) * dx;
        let zone = ((center + 180.0) / 360.0).floor();
        let end = ((((180.0 + zone * 360.0 - b[0] as f64) / dx) - 0.5).ceil() as usize)
            .clamp(start + 1, size[0]);
        let west = b[0] as f64 + start as f64 * dx - zone * 360.0;
        let east = b[0] as f64 + end as f64 * dx - zone * 360.0;
        let output = scratch.join(format!("{name}-{start}.bil"));
        let mut command = Command::new(gdal::gdal_tool_path("gdalwarp"));
        command
            .args([
                "-q",
                "-overwrite",
                "-of",
                "EHdr",
                "-ot",
                "Float32",
                "-t_srs",
                "EPSG:4326",
                "-te_srs",
                "EPSG:4326",
                "-te",
            ])
            .args([
                west.to_string(),
                b[1].to_string(),
                east.to_string(),
                b[3].to_string(),
            ])
            .args([
                "-ts",
                &(end - start).to_string(),
                &size[1].to_string(),
                "-r",
                "bilinear",
                "-et",
                "0",
                "-dstnodata",
                "-999999",
                "-wm",
                "64",
                "-wo",
                "NUM_THREADS=1",
                "--config",
                "GDAL_CACHEMAX",
                "64",
            ])
            .args(files)
            .arg(&output)
            .stdout(Stdio::null());
        gdal::run_command_with_timeout(command, "Atlas native elevation", Duration::from_secs(120))
            .ok()?;
        let bytes = std::fs::read(&output).ok()?;
        if bytes.len() != (end - start) * size[1] * 4 {
            return None;
        }
        for (i, bytes) in bytes.chunks_exact(4).enumerate() {
            out[(i / (end - start)) * size[0] + start + i % (end - start)] =
                f32::from_le_bytes(bytes.try_into().ok()?);
        }
        start = end;
    }
    Some(out)
}

#[cfg(test)]
#[path = "atlas_source_tests.rs"]
mod tests;
