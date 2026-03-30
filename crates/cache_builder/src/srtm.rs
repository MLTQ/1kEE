/// Minimal SRTM elevation sampler for the cache builder.
///
/// Loads GeoTIFF tiles on demand and caches up to `MAX_CACHED_TILES` in
/// memory.  Bilinearly interpolates between the four nearest grid samples.
/// Returns `0.0` for no-data cells (SRTM sentinel −32768) and for tiles
/// that cannot be opened (e.g. ocean areas with no tile file).
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const MAX_CACHED_TILES: usize = 8;

struct SrtmTile {
    width: u32,
    height: u32,
    /// Elevation values in metres, row-major, top row first (north to south).
    /// No-data cells are pre-converted to `0.0`.
    samples: Vec<f32>,
}

struct CachedEntry {
    path: PathBuf,
    tile: SrtmTile,
}

pub struct SrtmSampler {
    root: PathBuf,
    tiles: Vec<CachedEntry>, // front = most-recently used
    missing: HashSet<PathBuf>,
}

impl SrtmSampler {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tiles: Vec::new(),
            missing: HashSet::new(),
        }
    }

    /// Sample the elevation at `(lat, lon)` in metres.  Returns `0.0` if the
    /// tile file is absent or the point falls on a no-data cell.
    pub fn sample(&mut self, lat: f32, lon: f32) -> f32 {
        let path = tile_path(&self.root, lat, lon);

        if self.missing.contains(&path) {
            return 0.0;
        }

        // LRU hit.
        if let Some(idx) = self.tiles.iter().position(|e| e.path == path) {
            let entry = self.tiles.remove(idx);
            let value = entry.tile.sample(lat, lon);
            self.tiles.insert(0, entry);
            return value;
        }

        // Load tile from disk.
        let Some(tile) = load_tile(&path) else {
            self.missing.insert(path);
            return 0.0;
        };
        let value = tile.sample(lat, lon);
        self.tiles.insert(0, CachedEntry { path, tile });
        if self.tiles.len() > MAX_CACHED_TILES {
            self.tiles.pop();
        }
        value
    }
}

// ── Tile loading ──────────────────────────────────────────────────────────────

fn load_tile(path: &Path) -> Option<SrtmTile> {
    use tiff::decoder::{Decoder, DecodingResult};
    use std::fs::File;

    let file = File::open(path).ok()?;
    let mut decoder = Decoder::new(file).ok()?;
    let (width, height) = decoder.dimensions().ok()?;
    let result = decoder.read_image().ok()?;

    // Convert all supported SRTM pixel formats to f32 elevation values.
    // Nodata sentinel (-32768 for Int/UInt, NaN/-32768.0 for float) → 0.0.
    let samples: Vec<f32> = match result {
        DecodingResult::I16(data) => data
            .into_iter()
            .map(|s| if s == i16::MIN { 0.0 } else { s as f32 })
            .collect(),
        DecodingResult::U16(data) => data
            .into_iter()
            .map(|u| {
                let s = u as i16;
                if s == i16::MIN { 0.0 } else { s as f32 }
            })
            .collect(),
        DecodingResult::F32(data) => data
            .into_iter()
            .map(|f| if f.is_nan() || f <= -32767.0 { 0.0 } else { f })
            .collect(),
        DecodingResult::F64(data) => data
            .into_iter()
            .map(|f| if f.is_nan() || f <= -32767.0 { 0.0 } else { f as f32 })
            .collect(),
        _ => return None,
    };

    if samples.len() != (width * height) as usize {
        return None;
    }

    Some(SrtmTile { width, height, samples })
}

fn tile_path(root: &Path, lat: f32, lon: f32) -> PathBuf {
    let lat_base = lat.floor() as i32;
    let lon_base = lon.floor() as i32;
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

// ── Sampling ──────────────────────────────────────────────────────────────────

impl SrtmTile {
    fn sample(&self, lat: f32, lon: f32) -> f32 {
        let lat_base = lat.floor();
        let lon_base = lon.floor();
        let u = (lon - lon_base).clamp(0.0, 0.999_999);
        let v = (1.0 - (lat - lat_base)).clamp(0.0, 0.999_999);

        let x = u * self.width.saturating_sub(1) as f32;
        let y = v * self.height.saturating_sub(1) as f32;

        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(self.width.saturating_sub(1));
        let y1 = (y0 + 1).min(self.height.saturating_sub(1));
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;

        let tl = self.get(x0, y0);
        let tr = self.get(x1, y0);
        let bl = self.get(x0, y1);
        let br = self.get(x1, y1);

        let top = tl + (tr - tl) * tx;
        let bot = bl + (br - bl) * tx;
        top + (bot - top) * ty
    }

    fn get(&self, x: u32, y: u32) -> f32 {
        self.samples.get((y * self.width + x) as usize).copied().unwrap_or(0.0)
    }
}
