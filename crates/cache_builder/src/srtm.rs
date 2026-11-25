/// Minimal SRTM elevation sampler for the cache builder.
///
/// Loads GeoTIFF tiles on demand and caches up to `MAX_CACHED_TILES` in
/// memory, shared safely across Rayon cell-flushing workers.  Each handle keeps
/// a lock-free 1-slot fast cache for the current 1°×1° tile so consecutive
/// vertices in the same cell incur zero mutex acquisitions and zero allocations.
/// Returns `0.0` for no-data cells (SRTM sentinel −32768) and for tiles
/// that cannot be opened (e.g. ocean areas with no tile file).
use hashlink::lru_cache::LruCache;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_CACHED_TILES: usize = 32;

enum SrtmSamples {
    I16(Vec<i16>),
    F32(Vec<f32>),
}

struct SrtmTile {
    width: u32,
    height: u32,
    /// Elevation values in metres, row-major, top row first (north to south).
    samples: SrtmSamples,
}

type SharedCache = Arc<Mutex<LruCache<(i32, i32), Option<Arc<SrtmTile>>>>>;

pub struct SrtmSampler {
    root: Arc<PathBuf>,
    shared: SharedCache,
    /// Lock-free 1-slot fast cache for the most recent 1°×1° tile.
    last: Option<((i32, i32), Option<Arc<SrtmTile>>)>,
}

impl SrtmSampler {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
            shared: Arc::new(Mutex::new(LruCache::new(MAX_CACHED_TILES))),
            last: None,
        }
    }

    /// Cheap handle sharing the underlying tile LRU across Rayon workers.
    pub fn clone_handle(&self) -> Self {
        Self {
            root: Arc::clone(&self.root),
            shared: Arc::clone(&self.shared),
            last: None,
        }
    }

    /// Sample the elevation at `(lat, lon)` in metres.  Returns `0.0` if the
    /// tile file is absent or the point falls on a no-data cell.
    #[inline]
    pub fn sample(&mut self, lat: f32, lon: f32) -> f32 {
        let key = (lat.floor() as i32, lon.floor() as i32);

        if let Some((last_key, ref tile)) = self.last
            && last_key == key
        {
            return match tile {
                Some(t) => t.sample(lat, lon),
                None => 0.0,
            };
        }

        let tile = {
            let mut guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(entry) = guard.get(&key) {
                entry.clone()
            } else {
                drop(guard);
                let loaded = load_tile(&tile_path(&self.root, key.0, key.1)).map(Arc::new);
                let mut guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
                guard.insert(key, loaded.clone());
                loaded
            }
        };

        let value = match &tile {
            Some(t) => t.sample(lat, lon),
            None => 0.0,
        };
        self.last = Some((key, tile));
        value
    }
}

// ── Tile loading ──────────────────────────────────────────────────────────────

fn load_tile(path: &Path) -> Option<SrtmTile> {
    use std::fs::File;
    use tiff::decoder::{Decoder, DecodingResult};

    let file = File::open(path).ok()?;
    let mut decoder = Decoder::new(BufReader::with_capacity(256 * 1024, file)).ok()?;
    let (width, height) = decoder.dimensions().ok()?;
    let result = decoder.read_image().ok()?;

    let expected = (width as usize).checked_mul(height as usize)?;
    let samples = match result {
        DecodingResult::I16(data) => {
            if data.len() != expected {
                return None;
            }
            SrtmSamples::I16(data)
        }
        DecodingResult::U16(data) => {
            if data.len() != expected {
                return None;
            }
            SrtmSamples::I16(data.into_iter().map(|u| u as i16).collect())
        }
        DecodingResult::F32(mut data) => {
            if data.len() != expected {
                return None;
            }
            for f in &mut data {
                if f.is_nan() || *f <= -32767.0 {
                    *f = 0.0;
                }
            }
            SrtmSamples::F32(data)
        }
        DecodingResult::F64(data) => {
            if data.len() != expected {
                return None;
            }
            SrtmSamples::F32(
                data.into_iter()
                    .map(|f| {
                        if f.is_nan() || f <= -32767.0 {
                            0.0
                        } else {
                            f as f32
                        }
                    })
                    .collect(),
            )
        }
        _ => return None,
    };

    Some(SrtmTile {
        width,
        height,
        samples,
    })
}

fn tile_path(root: &Path, lat_base: i32, lon_base: i32) -> PathBuf {
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
    #[inline]
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

    #[inline]
    fn get(&self, x: u32, y: u32) -> f32 {
        let idx = (y * self.width + x) as usize;
        match &self.samples {
            SrtmSamples::I16(v) => v
                .get(idx)
                .copied()
                .map_or(0.0, |s| if s == i16::MIN { 0.0 } else { s as f32 }),
            SrtmSamples::F32(v) => v.get(idx).copied().unwrap_or(0.0),
        }
    }
}
