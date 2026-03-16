use crate::model::GeoPoint;
use crate::terrain_assets;
use image::ImageReader;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MIN_LAND_ELEVATION_M: f32 = -600.0;
const MAX_LAND_ELEVATION_M: f32 = 9_000.0;
const MAX_CACHED_TILES: usize = 8;

struct SrtmTile {
    width: u32,
    height: u32,
    samples: Vec<u16>,
}

struct CachedTile {
    path: PathBuf,
    tile: SrtmTile,
}

struct TileCache {
    tiles: Vec<CachedTile>,
    missing: HashSet<PathBuf>,
}

pub fn sample_normalized(selected_root: Option<&Path>, point: GeoPoint) -> Option<f32> {
    let elevation_m = sample_elevation_m(selected_root, point)?;
    Some(
        ((elevation_m - MIN_LAND_ELEVATION_M) / (MAX_LAND_ELEVATION_M - MIN_LAND_ELEVATION_M))
            .clamp(0.0, 1.0),
    )
}

pub fn sample_elevation_m(selected_root: Option<&Path>, point: GeoPoint) -> Option<f32> {
    let root = terrain_assets::find_srtm_root(selected_root)?;
    let path = tile_path(&root, point);

    static CACHE: OnceLock<Mutex<TileCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        Mutex::new(TileCache {
            tiles: Vec::new(),
            missing: HashSet::new(),
        })
    });
    let mut guard = cache.lock().ok()?;

    if guard.missing.contains(&path) {
        return None;
    }

    if let Some(index) = guard.tiles.iter().position(|tile| tile.path == path) {
        let cached = guard.tiles.remove(index);
        let value = cached.tile.sample_elevation_m(point);
        guard.tiles.insert(0, cached);
        return Some(value);
    }

    let tile = match load_tile(path.clone()) {
        Some(tile) => tile,
        None => {
            guard.missing.insert(path);
            return None;
        }
    };

    let value = tile.sample_elevation_m(point);
    guard.tiles.insert(0, CachedTile { path, tile });
    if guard.tiles.len() > MAX_CACHED_TILES {
        guard.tiles.pop();
    }

    Some(value)
}

impl SrtmTile {
    fn sample_elevation_m(&self, point: GeoPoint) -> f32 {
        let lat_base = point.lat.floor();
        let lon_base = point.lon.floor();
        let u = (point.lon - lon_base).clamp(0.0, 0.999_999);
        let v = (1.0 - (point.lat - lat_base)).clamp(0.0, 0.999_999);

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
        lerp(top, bottom, ty)
    }
}

fn load_tile(path: PathBuf) -> Option<SrtmTile> {
    let image = ImageReader::open(path).ok()?.decode().ok()?.to_luma16();
    let (width, height) = image.dimensions();

    Some(SrtmTile {
        width,
        height,
        samples: image.into_raw(),
    })
}

fn tile_path(root: &Path, point: GeoPoint) -> PathBuf {
    let lat_base = point.lat.floor() as i32;
    let lon_base = point.lon.floor() as i32;
    let lat_prefix = if lat_base >= 0 { 'N' } else { 'S' };
    let lon_prefix = if lon_base >= 0 { 'E' } else { 'W' };

    root.join(format!(
        "{}{:02}{}{:03}.tif",
        lat_prefix,
        lat_base.unsigned_abs(),
        lon_prefix,
        lon_base.unsigned_abs()
    ))
}

fn sample_pixel(tile: &SrtmTile, x: u32, y: u32) -> f32 {
    let index = (y * tile.width + x) as usize;
    let raw = tile.samples.get(index).copied().unwrap_or_default();
    let signed = raw as i16;
    if signed == i16::MIN {
        0.0
    } else {
        signed as f32
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
