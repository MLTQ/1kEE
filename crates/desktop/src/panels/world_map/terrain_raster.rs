use crate::model::GeoPoint;
use crate::terrain_assets;
use image::ImageReader;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::srtm_stream;

const MIN_ELEVATION_M: f32 = -11_000.0;
const MAX_ELEVATION_M: f32 = 9_000.0;

pub struct TerrainRaster {
    width: u32,
    height: u32,
    pixels: Vec<u16>,
}

struct CachedRaster {
    path: PathBuf,
    raster: TerrainRaster,
}

impl TerrainRaster {
    pub fn sample_normalized(&self, point: GeoPoint) -> f32 {
        let elevation_m = self.sample_elevation_m(point);
        ((elevation_m - MIN_ELEVATION_M) / (MAX_ELEVATION_M - MIN_ELEVATION_M)).clamp(0.0, 1.0)
    }

    fn sample_elevation_m(&self, point: GeoPoint) -> f32 {
        let u = ((point.lon + 180.0) / 360.0).rem_euclid(1.0);
        let v = ((90.0 - point.lat) / 180.0).clamp(0.0, 1.0);

        let x = u * (self.width.saturating_sub(1)) as f32;
        let y = v * (self.height.saturating_sub(1)) as f32;

        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(self.width.saturating_sub(1));
        let y1 = (y0 + 1).min(self.height.saturating_sub(1));
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;

        let top = lerp(sample_pixel(self, x0, y0), sample_pixel(self, x1, y0), tx);
        let bottom = lerp(sample_pixel(self, x0, y1), sample_pixel(self, x1, y1), tx);
        let normalized = lerp(top, bottom, ty);

        MIN_ELEVATION_M + normalized * (MAX_ELEVATION_M - MIN_ELEVATION_M)
    }
}

pub fn sample_normalized(selected_root: Option<&Path>, point: GeoPoint) -> Option<f32> {
    if let Some(value) = srtm_stream::sample_normalized(selected_root, point) {
        return Some(value);
    }

    let path = terrain_assets::find_derived_root(selected_root)?
        .join("terrain/gebco_2025_preview_4096.png");

    static CACHE: OnceLock<Mutex<Option<CachedRaster>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if guard
        .as_ref()
        .map(|cached| cached.path.as_path() != path.as_path())
        .unwrap_or(true)
    {
        let raster = load_raster(path.clone())?;
        *guard = Some(CachedRaster { path, raster });
    }

    guard
        .as_ref()
        .map(|cached| cached.raster.sample_normalized(point))
}

fn load_raster(path: PathBuf) -> Option<TerrainRaster> {
    let image = ImageReader::open(path).ok()?.decode().ok()?.to_luma16();
    let (width, height) = image.dimensions();
    let pixels = image.into_raw();

    Some(TerrainRaster {
        width,
        height,
        pixels,
    })
}

fn sample_pixel(raster: &TerrainRaster, x: u32, y: u32) -> f32 {
    let index = (y * raster.width + x) as usize;
    raster
        .pixels
        .get(index)
        .map(|value| *value as f32 / u16::MAX as f32)
        .unwrap_or_default()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
