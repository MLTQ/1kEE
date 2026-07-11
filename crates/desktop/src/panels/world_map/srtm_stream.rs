use crate::model::GeoPoint;
use crate::terrain_assets;
use image::ImageReader;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[allow(dead_code)]
const MIN_LAND_ELEVATION_M: f32 = -600.0;
#[allow(dead_code)]
const MAX_LAND_ELEVATION_M: f32 = 9_000.0;
const MAX_CACHED_TILES: usize = 8;
/// Raw TIFF decode (or GDAL normalization) is intentionally serialized for
/// interactive marker preloads. Exact background layer sampling retains its
/// existing independent path, while local paint keeps a core free.
const MAX_CONCURRENT_MARKER_PRELOADS: usize = 1;

struct SrtmTile {
    width: u32,
    height: u32,
    samples: Vec<i16>,
}

struct CachedTile {
    path: PathBuf,
    tile: SrtmTile,
}

struct TileCache {
    tiles: Vec<CachedTile>,
    missing: HashSet<PathBuf>,
    /// Tile rasters currently being decoded or normalized by a preload worker.
    /// This is path-keyed so dozens of visible markers cannot launch duplicate
    /// TIFF decodes or GDAL subprocesses for the same one-degree tile.
    loading: HashSet<PathBuf>,
}

fn tile_cache() -> &'static Mutex<TileCache> {
    static CACHE: OnceLock<Mutex<TileCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(TileCache {
            tiles: Vec::new(),
            missing: HashSet::new(),
            loading: HashSet::new(),
        })
    })
}

#[allow(dead_code)]
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

    let cache = tile_cache();
    // Block 1: Lock to check cache
    {
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
    }

    // Block 2: Load freely unlocked, avoiding UI halting or deadlock contention!
    let tile = match load_tile(path.clone()) {
        Some(tile) => tile,
        None => {
            if let Ok(mut guard) = cache.lock() {
                guard.missing.insert(path);
            }
            return None;
        }
    };

    let value = tile.sample_elevation_m(point);

    // Block 3: Re-acquire to append
    if let Ok(mut guard) = cache.lock() {
        // Check once more in case another thread simultaneously loaded it to prevent duplicates
        if !guard.tiles.iter().any(|t| t.path == path) {
            guard.tiles.insert(0, CachedTile { path, tile });
            if guard.tiles.len() > MAX_CACHED_TILES {
                guard.tiles.pop();
            }
        }
    }

    Some(value)
}

/// Return a loaded SRTM sample without ever opening a raster or waiting for
/// GDAL. Local marker paint uses this path, preserving the blocking sampler
/// above for background layer builders that require an exact elevation.
pub fn peek_elevation_m(selected_root: Option<&Path>, point: GeoPoint) -> Option<f32> {
    let root = terrain_assets::find_srtm_root(selected_root)?;
    let path = tile_path(&root, point);
    let mut cache = tile_cache().lock().ok()?;

    if cache.missing.contains(&path) {
        return None;
    }
    let index = cache.tiles.iter().position(|tile| tile.path == path)?;
    let cached = cache.tiles.remove(index);
    let value = cached.tile.sample_elevation_m(point);
    cache.tiles.insert(0, cached);
    Some(value)
}

/// Start a deduplicated background SRTM tile load. The first local marker in
/// a new tile renders at its existing zero-terrain fallback for one or more
/// frames; a repaint replaces it with the exact cached elevation once ready.
pub fn request_elevation_preload(selected_root: Option<&Path>, point: GeoPoint) {
    let Some(root) = terrain_assets::find_srtm_root(selected_root) else {
        return;
    };
    let path = tile_path(&root, point);
    let cache = tile_cache();
    {
        let Ok(mut cache) = cache.lock() else {
            return;
        };
        if cache.missing.contains(&path)
            || cache.tiles.iter().any(|tile| tile.path == path)
            || cache.loading.len() >= MAX_CONCURRENT_MARKER_PRELOADS
            || !cache.loading.insert(path.clone())
        {
            return;
        }
    }

    let cleanup_path = path.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("srtm-elevation-preload".into())
        .spawn(move || {
            let tile = load_tile(path.clone());
            if let Ok(mut cache) = cache.lock() {
                cache.loading.remove(&path);
                if let Some(tile) = tile {
                    if !cache.tiles.iter().any(|cached| cached.path == path) {
                        cache.tiles.insert(0, CachedTile { path, tile });
                        if cache.tiles.len() > MAX_CACHED_TILES {
                            cache.tiles.pop();
                        }
                    }
                } else {
                    cache.missing.insert(path);
                }
            }
            crate::app::request_repaint();
        })
    {
        if let Ok(mut cache) = cache.lock() {
            cache.loading.remove(&cleanup_path);
        }
        eprintln!("[1kEE] failed to spawn SRTM elevation preload: {error}");
    }
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

// ── Tile loading ───────────────────────────────────────────────────────────────
//
// Two-stage: try the image crate first (zero subprocess overhead).  If that
// fails — which happens with Float32 GeoTIFFs, unusual compression, or other
// variants that GDAL reads but the image crate does not — fall back to
// `gdal_translate` to normalise the tile to a simple uncompressed Int16 ENVI
// raw file, then read that directly.  Normalised tiles are cached in the system
// temp directory so the subprocess only runs once per source tile.

fn load_tile(path: PathBuf) -> Option<SrtmTile> {
    // Fast path: image crate handles standard Int16 / UInt16 GeoTIFFs.
    if let Some(tile) = load_tile_via_image(&path) {
        return Some(tile);
    }
    // Slow-but-reliable path: convert via gdal_translate, cache result on disk.
    load_tile_via_gdal(&path)
}

fn load_tile_via_image(path: &Path) -> Option<SrtmTile> {
    let image = ImageReader::open(path).ok()?.decode().ok()?.to_luma16();
    let (width, height) = image.dimensions();
    // Samples from the image crate come back as u16; reinterpret as i16 so
    // that negative elevations and the SRTM no-data sentinel (-32768) work.
    let samples: Vec<i16> = image.into_raw().into_iter().map(|u| u as i16).collect();
    Some(SrtmTile {
        width,
        height,
        samples,
    })
}

/// Convert `src` to a headerless raw Int16 little-endian binary via
/// `gdal_translate`, then read it.  The result is cached alongside the source
/// file as `<src_stem>.srtm_raw` so subsequent cold-starts skip the subprocess.
fn load_tile_via_gdal(src: &Path) -> Option<SrtmTile> {
    let raw_path = src.with_extension("srtm_raw");

    // Re-use a previous conversion if it exists.
    if raw_path.exists() {
        if let Some(tile) = load_raw_int16(&raw_path) {
            return Some(tile);
        }
    }

    // gdal_translate -ot Int16 -of ENVI produces a .img raw file + .hdr header.
    // We use -of EHdr (ESRI BIL) which generates a .bil + .hdr pair instead and
    // is more reliably available.  The output stem is the raw_path without
    // extension so gdal_translate appends its own extensions.
    let stem = raw_path.with_extension("");
    let hdr_path = stem.with_extension("hdr");
    let bil_path = stem.with_extension("bil");

    let gdal_translate = crate::settings_store::resolve_gdal_tool("gdal_translate");
    let status = std::process::Command::new(&gdal_translate)
        .args(["-q", "-ot", "Int16", "-of", "EHdr"])
        .arg(src)
        .arg(&stem)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;

    if !status.success() {
        return None;
    }

    // Parse width/height from the EHdr .hdr file.
    let hdr_text = std::fs::read_to_string(&hdr_path).ok()?;
    let (mut width, mut height, mut big_endian) = (0u32, 0u32, false);
    for line in hdr_text.lines() {
        let line = line.trim();
        if let Some(val) = line
            .strip_prefix("NCOLS")
            .or_else(|| line.strip_prefix("ncols"))
        {
            width = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = line
            .strip_prefix("NROWS")
            .or_else(|| line.strip_prefix("nrows"))
        {
            height = val.trim().parse().unwrap_or(0);
        } else if line.starts_with("BYTEORDER") || line.starts_with("byteorder") {
            let val = line.split_whitespace().nth(1).unwrap_or("I");
            big_endian = val == "M";
        }
    }

    if width == 0 || height == 0 {
        return None;
    }

    // Read raw Int16 bytes from the .bil file.
    let raw_bytes = std::fs::read(&bil_path).ok()?;
    let expected = (width * height * 2) as usize;
    if raw_bytes.len() < expected {
        return None;
    }

    let samples: Vec<i16> = raw_bytes
        .chunks_exact(2)
        .take((width * height) as usize)
        .map(|b| {
            let arr = [b[0], b[1]];
            if big_endian {
                i16::from_be_bytes(arr)
            } else {
                i16::from_le_bytes(arr)
            }
        })
        .collect();

    // Write a compact cache file (raw LE Int16 + 8-byte header: u32 width, u32 height).
    if let Ok(mut f) = std::fs::File::create(&raw_path) {
        use std::io::Write;
        let _ = f.write_all(&width.to_le_bytes());
        let _ = f.write_all(&height.to_le_bytes());
        for &s in &samples {
            let _ = f.write_all(&s.to_le_bytes());
        }
    }
    let _ = std::fs::remove_file(&hdr_path);
    let _ = std::fs::remove_file(&bil_path);

    Some(SrtmTile {
        width,
        height,
        samples,
    })
}

/// Load a tile from the compact `.srtm_raw` cache format:
/// 4-byte LE u32 width, 4-byte LE u32 height, then width*height LE i16 samples.
fn load_raw_int16(path: &Path) -> Option<SrtmTile> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 8 {
        return None;
    }
    let width = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let height = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let expected = 8 + (width * height * 2) as usize;
    if bytes.len() < expected {
        return None;
    }
    let samples: Vec<i16> = bytes[8..]
        .chunks_exact(2)
        .take((width * height) as usize)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    Some(SrtmTile {
        width,
        height,
        samples,
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
    let signed = tile.samples.get(index).copied().unwrap_or(i16::MIN);
    if signed == i16::MIN {
        0.0 // SRTM no-data sentinel
    } else {
        signed as f32
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
