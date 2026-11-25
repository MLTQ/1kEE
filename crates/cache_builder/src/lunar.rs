/// Offline pre-builder for SLDEM2015 lunar contour tiles.
///
/// The desktop app builds these on-demand from the single 22 GB SLDEM2015 JP2
/// file, which is very slow (minutes per tile).  This module pre-builds the
/// entire bbox into `lunar_focus_cache.sqlite` so the desktop finds them
/// instantly and skips its own build.
///
/// Pipeline:
///   gdal_translate (once per source chunk)                    → cached Int16 GeoTIFF
///   native marching squares (many tiles in parallel)          → contour polylines
///   SQLite writer thread                                      → lunar_focus_cache.sqlite
use crate::contours::{
    ContourBuildProgress, FocusContourSpec, GeoBounds, TileKey, bucket_range, open_cache_db,
    resolve_gdal_tool,
};
use crate::marching_squares::build_tile_contours_with_sampler;
use rayon::prelude::*;
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// ── Lunar zoom specs (matches desktop zoom::lunar_spec_for_zoom exactly) ─────

#[derive(Clone, Copy)]
pub struct LunarSpec {
    pub half_extent_deg: f32,
    pub raster_size: u32,
    pub interval_m: i32,
    pub zoom_bucket: i32,
}

pub fn all_lunar_specs() -> [LunarSpec; 5] {
    [
        LunarSpec {
            half_extent_deg: 3.6,
            raster_size: 384,
            interval_m: 1000,
            zoom_bucket: 0,
        },
        LunarSpec {
            half_extent_deg: 2.2,
            raster_size: 512,
            interval_m: 500,
            zoom_bucket: 1,
        },
        LunarSpec {
            half_extent_deg: 1.4,
            raster_size: 576,
            interval_m: 200,
            zoom_bucket: 2,
        },
        LunarSpec {
            half_extent_deg: 0.9,
            raster_size: 640,
            interval_m: 100,
            zoom_bucket: 3,
        },
        LunarSpec {
            half_extent_deg: 0.55,
            raster_size: 704,
            interval_m: 50,
            zoom_bucket: 4,
        },
    ]
}

fn focus_spec(spec: LunarSpec) -> FocusContourSpec {
    FocusContourSpec {
        half_extent_deg: spec.half_extent_deg,
        raster_size: spec.raster_size,
        interval_m: spec.interval_m,
        zoom_bucket: spec.zoom_bucket,
    }
}

const SOURCE_CHUNK_CENTER_STEP_DEG: f32 = 4.0;
const SOURCE_CHUNK_HALF_EXTENT_DEG: f32 = 6.0;
const SOURCE_CHUNK_DIR_NAME: &str = "lunar_source_chunks";

// ── Command ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LunarBuildCommand {
    pub jp2_path: PathBuf,
    pub cache_db_path: PathBuf, // path to lunar_focus_cache.sqlite
    pub tmp_dir: Option<PathBuf>,
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
    pub zoom_buckets: Vec<i32>, // subset of 0..=4
    pub gdal_bin_dir: PathBuf,  // "" = use Homebrew / $PATH
}

// ── GDAL helpers ──────────────────────────────────────────────────────────────

fn run_gdal_with_timeout(mut cmd: Command, label: &str, timeout: Duration) -> std::io::Result<()> {
    let start = Instant::now();
    let mut child = cmd.spawn()?;
    let mut poll_ms = 2u64;
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "{label} failed with status {status}"
                )))
            };
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{label} timed out after {:?}", timeout),
            ));
        }
        std::thread::sleep(Duration::from_millis(poll_ms));
        poll_ms = (poll_ms * 2).min(50);
    }
}

/// Like `run_gdal` in contours.rs but with a 10-minute timeout — reading a
/// geographic subregion from the 22 GB SLDEM JP2 can take several minutes.
fn run_gdal_jp2(cmd: Command, label: &str) -> std::io::Result<()> {
    run_gdal_with_timeout(cmd, label, Duration::from_secs(600))
}

#[derive(Clone)]
struct SourceChunk {
    path: PathBuf,
    bounds: GeoBounds,
    raster_size: u32,
    spec: LunarSpec,
}

fn source_chunk_root(cache_db_path: &Path) -> PathBuf {
    cache_db_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(SOURCE_CHUNK_DIR_NAME)
}

fn source_chunk_for_bounds(
    cache_db_path: &Path,
    bounds: GeoBounds,
    spec: LunarSpec,
) -> SourceChunk {
    let center_lat = (bounds.min_lat + bounds.max_lat) * 0.5;
    let center_lon = (bounds.min_lon + bounds.max_lon) * 0.5;
    let lat_bucket = (center_lat / SOURCE_CHUNK_CENTER_STEP_DEG).round() as i32;
    let lon_bucket = (center_lon / SOURCE_CHUNK_CENTER_STEP_DEG).round() as i32;
    let chunk_center_lat = lat_bucket as f32 * SOURCE_CHUNK_CENTER_STEP_DEG;
    let chunk_center_lon = lon_bucket as f32 * SOURCE_CHUNK_CENTER_STEP_DEG;
    let pixels_per_degree = spec.raster_size as f32 / (spec.half_extent_deg * 2.0);
    let chunk_span = SOURCE_CHUNK_HALF_EXTENT_DEG * 2.0;
    let raster_size = (chunk_span * pixels_per_degree).ceil() as u32;
    let dir = source_chunk_root(cache_db_path).join(format!("z{}", spec.zoom_bucket));
    let file_name = format!("lat{lat_bucket:+04}_lon{lon_bucket:+04}.tif");
    SourceChunk {
        path: dir.join(file_name),
        bounds: GeoBounds {
            min_lat: (chunk_center_lat - SOURCE_CHUNK_HALF_EXTENT_DEG).clamp(-89.999, 89.999),
            max_lat: (chunk_center_lat + SOURCE_CHUNK_HALF_EXTENT_DEG).clamp(-89.999, 89.999),
            min_lon: chunk_center_lon - SOURCE_CHUNK_HALF_EXTENT_DEG,
            max_lon: chunk_center_lon + SOURCE_CHUNK_HALF_EXTENT_DEG,
        },
        raster_size: raster_size.max(spec.raster_size),
        spec,
    }
}

fn temp_sibling(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("chunk");
    path.with_file_name(format!("{stem}.{suffix}"))
}

fn persist_temp_file(tmp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    if fs::rename(tmp_path, final_path).is_ok() {
        return Ok(());
    }
    fs::copy(tmp_path, final_path)?;
    fs::remove_file(tmp_path)?;
    Ok(())
}

fn ensure_source_chunk(
    jp2_path: &Path,
    cache_db_path: &Path,
    gdal_translate: &Path,
    spec: LunarSpec,
    bounds: GeoBounds,
) -> Result<SourceChunk, String> {
    let chunk = source_chunk_for_bounds(cache_db_path, bounds, spec);
    if chunk.path.exists() {
        return Ok(chunk);
    }

    let Some(parent) = chunk.path.parent() else {
        return Err(format!(
            "Invalid lunar source chunk path: {}",
            chunk.path.display()
        ));
    };
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    let tmp_chunk = temp_sibling(&chunk.path, &format!("{}.tmp.tif", std::process::id()));
    let _ = fs::remove_file(&tmp_chunk);

    let mut translate = Command::new(gdal_translate);
    translate.args([
        "-q",
        "-projwin",
        &chunk.bounds.min_lon.to_string(),
        &chunk.bounds.max_lat.to_string(),
        &chunk.bounds.max_lon.to_string(),
        &chunk.bounds.min_lat.to_string(),
        "-outsize",
        &chunk.raster_size.to_string(),
        &chunk.raster_size.to_string(),
        "-scale",
        "-18000",
        "22000",
        "-9000",
        "11000",
        "-ot",
        "Int16",
        "-of",
        "GTiff",
        "-co",
        "TILED=YES",
        "-co",
        "COMPRESS=LZW",
        "-co",
        "PREDICTOR=2",
        "-co",
        "BLOCKXSIZE=512",
        "-co",
        "BLOCKYSIZE=512",
    ]);
    translate.env("GDAL_NUM_THREADS", "ALL_CPUS");
    translate.env("OPJ_NUM_THREADS", "ALL_CPUS");
    translate.arg(jp2_path).arg(&tmp_chunk);
    run_gdal_jp2(translate, "gdal_translate (lunar source chunk)").map_err(|e| e.to_string())?;
    persist_temp_file(&tmp_chunk, &chunk.path).map_err(|e| e.to_string())?;
    Ok(chunk)
}

enum LunarSamples {
    I16(Vec<i16>),
    F32(Vec<f32>),
}

struct LunarChunkRaster {
    width: u32,
    height: u32,
    bounds: GeoBounds,
    samples: LunarSamples,
}

impl LunarChunkRaster {
    fn load(path: &Path, bounds: GeoBounds) -> Option<Self> {
        use std::fs::File;
        use std::io::BufReader;
        use tiff::decoder::{Decoder, DecodingResult};

        let file = File::open(path)
            .map_err(|e| eprintln!("[1kEE] lunar chunk open error {}: {e}", path.display()))
            .ok()?;
        let mut decoder = Decoder::new(BufReader::with_capacity(256 * 1024, file))
            .map_err(|e| eprintln!("[1kEE] lunar chunk TIFF init error {}: {e}", path.display()))
            .ok()?;
        let (width, height) = decoder
            .dimensions()
            .map_err(|e| {
                eprintln!(
                    "[1kEE] lunar chunk dimensions error {}: {e}",
                    path.display()
                )
            })
            .ok()?;
        let result = decoder
            .read_image()
            .map_err(|e| eprintln!("[1kEE] lunar chunk read error {}: {e}", path.display()))
            .ok()?;

        let samples = match result {
            DecodingResult::I16(data) => LunarSamples::I16(data),
            DecodingResult::U16(data) => {
                LunarSamples::I16(data.into_iter().map(|u| u as i16).collect())
            }
            DecodingResult::F32(data) => LunarSamples::F32(data),
            DecodingResult::F64(data) => {
                LunarSamples::F32(data.into_iter().map(|f| f as f32).collect())
            }
            _ => {
                eprintln!(
                    "[1kEE] lunar chunk unsupported pixel format in {}",
                    path.display()
                );
                return None;
            }
        };

        let len = match &samples {
            LunarSamples::I16(v) => v.len(),
            LunarSamples::F32(v) => v.len(),
        };
        if len != (width * height) as usize {
            eprintln!(
                "[1kEE] lunar chunk sample count mismatch {}: got {} expected {}",
                path.display(),
                len,
                width * height
            );
            return None;
        }

        Some(Self {
            width,
            height,
            bounds,
            samples,
        })
    }

    #[inline]
    fn get(&self, x: u32, y: u32) -> f32 {
        let idx = (y * self.width + x) as usize;
        match &self.samples {
            LunarSamples::I16(data) => data.get(idx).map(|&v| v as f32).unwrap_or(f32::NAN),
            LunarSamples::F32(data) => data.get(idx).copied().unwrap_or(f32::NAN),
        }
    }

    #[inline]
    fn sample(&self, lat: f32, lon: f32) -> f32 {
        let u = ((lon - self.bounds.min_lon) / (self.bounds.max_lon - self.bounds.min_lon))
            .clamp(0.0, 0.999_999);
        let v = ((self.bounds.max_lat - lat) / (self.bounds.max_lat - self.bounds.min_lat))
            .clamp(0.0, 0.999_999);

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
        let bottom = bl + (br - bl) * tx;
        top + (bottom - top) * ty
    }
}

type LunarChunkKey = (i32, i32, i32); // (zoom_bucket, lat_bucket, lon_bucket)

struct SharedLunarChunkCache {
    cache_db_path: PathBuf,
    lru: std::sync::Mutex<hashlink::LruCache<LunarChunkKey, Option<Arc<LunarChunkRaster>>>>,
}

impl SharedLunarChunkCache {
    fn new(cache_db_path: PathBuf, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            cache_db_path,
            lru: std::sync::Mutex::new(hashlink::LruCache::new(capacity.max(4))),
        })
    }

    fn get_or_load(&self, key: LunarChunkKey, spec: LunarSpec) -> Option<Arc<LunarChunkRaster>> {
        if let Ok(mut guard) = self.lru.lock() {
            if let Some(hit) = guard.get(&key) {
                return hit.clone();
            }
        }
        let (_, lat_bucket, lon_bucket) = key;
        let chunk_center_lat = lat_bucket as f32 * SOURCE_CHUNK_CENTER_STEP_DEG;
        let chunk_center_lon = lon_bucket as f32 * SOURCE_CHUNK_CENTER_STEP_DEG;
        let bounds = GeoBounds {
            min_lat: (chunk_center_lat - SOURCE_CHUNK_HALF_EXTENT_DEG).clamp(-89.999, 89.999),
            max_lat: (chunk_center_lat + SOURCE_CHUNK_HALF_EXTENT_DEG).clamp(-89.999, 89.999),
            min_lon: chunk_center_lon - SOURCE_CHUNK_HALF_EXTENT_DEG,
            max_lon: chunk_center_lon + SOURCE_CHUNK_HALF_EXTENT_DEG,
        };
        let path = source_chunk_root(&self.cache_db_path)
            .join(format!("z{}", spec.zoom_bucket))
            .join(format!("lat{lat_bucket:+04}_lon{lon_bucket:+04}.tif"));
        let loaded = LunarChunkRaster::load(&path, bounds).map(Arc::new);
        if let Ok(mut guard) = self.lru.lock() {
            guard.insert(key, loaded.clone());
        }
        loaded
    }
}

struct LunarChunkSampler {
    shared: Arc<SharedLunarChunkCache>,
    spec: LunarSpec,
    last: Option<(LunarChunkKey, Option<Arc<LunarChunkRaster>>)>,
}

impl LunarChunkSampler {
    fn new(shared: Arc<SharedLunarChunkCache>, spec: LunarSpec) -> Self {
        Self {
            shared,
            spec,
            last: None,
        }
    }

    #[inline]
    fn sample(&mut self, lat: f32, lon: f32) -> f32 {
        let lat_bucket = (lat / SOURCE_CHUNK_CENTER_STEP_DEG).round() as i32;
        let lon_bucket = (lon / SOURCE_CHUNK_CENTER_STEP_DEG).round() as i32;
        let key = (self.spec.zoom_bucket, lat_bucket, lon_bucket);
        if let Some((last_key, ref raster)) = self.last
            && last_key == key
        {
            return match raster {
                Some(r) => r.sample(lat, lon),
                None => f32::NAN,
            };
        }
        let raster = self.shared.get_or_load(key, self.spec);
        let value = match &raster {
            Some(r) => r.sample(lat, lon),
            None => f32::NAN,
        };
        self.last = Some((key, raster));
        value
    }
}

fn encode_gpkg_linestring(points: &[(f32, f32)]) -> Vec<u8> {
    let n = points.len() as u32;
    let mut buf = Vec::with_capacity(8 + 1 + 4 + 4 + (n as usize) * 16);
    buf.extend_from_slice(b"GP");
    buf.push(0);
    buf.push(0);
    buf.extend_from_slice(&4326i32.to_le_bytes());
    buf.push(0x01);
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&n.to_le_bytes());
    for &(lon, lat) in points {
        buf.extend_from_slice(&(lon as f64).to_le_bytes());
        buf.extend_from_slice(&(lat as f64).to_le_bytes());
    }
    buf
}

fn write_tile_native_lunar_encoded(
    conn: &mut Connection,
    tile: TileKey,
    encoded: &[(i64, f32, Vec<u8>)],
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM contour_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    tx.execute(
        "DELETE FROM contour_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;

    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO contour_tiles (zoom_bucket,lat_bucket,lon_bucket,fid,elevation_m,geom)
             VALUES (?1,?2,?3,?4,?5,?6)",
        )?;
        for (fid, elevation_m, blob) in encoded {
            insert.execute(params![
                tile.zoom_bucket,
                tile.lat_bucket,
                tile.lon_bucket,
                fid,
                elevation_m,
                blob.as_slice(),
            ])?;
        }
    }
    tx.execute(
        "INSERT INTO contour_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,contour_count,built_at)
         VALUES (?1,?2,?3,?4,unixepoch())",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket, encoded.len() as i64],
    )?;

    tx.execute(
        "DELETE FROM coastline_tiles WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    tx.execute(
        "DELETE FROM coastline_tile_manifest WHERE zoom_bucket=?1 AND lat_bucket=?2 AND lon_bucket=?3",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    tx.execute(
        "INSERT INTO coastline_tile_manifest (zoom_bucket,lat_bucket,lon_bucket,line_count)
         VALUES (?1,?2,?3,0)",
        params![tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket],
    )?;
    tx.commit()
}

// ── Main entry point ─────────────────────────────────────────────────────────

/// Pre-build lunar contour tiles for the given bounding box and zoom buckets.
///
/// Writes into `command.cache_db_path` (the desktop reads this as
/// `Derived/terrain/lunar_focus_cache.sqlite`).
///
/// Tiles already present in the DB are skipped.  Coverage is clipped to ±60°
/// latitude (the extent of SLDEM2015).
pub fn build_lunar_contour_tiles(
    command: LunarBuildCommand,
    progress: &mut dyn FnMut(ContourBuildProgress),
) -> Result<String, String> {
    if !command.jp2_path.exists() {
        return Err(format!(
            "SLDEM JP2 not found: {}",
            command.jp2_path.display()
        ));
    }
    let gdal_translate = resolve_gdal_tool(&command.gdal_bin_dir, "gdal_translate");
    for (tool, name) in [(&gdal_translate, "gdal_translate")] {
        match Command::new(tool).arg("--version").output() {
            Ok(out) if out.status.success() => {
                let ver = String::from_utf8_lossy(&out.stdout);
                progress(ContourBuildProgress::info(
                    "Startup",
                    0.0,
                    format!("{name}: {}", ver.trim()),
                ));
            }
            Ok(_) => return Err(format!("{name} at '{}' returned an error", tool.display())),
            Err(e) => {
                return Err(format!(
                    "Could not launch {name} at '{}': {e}. Set GDAL bin dir.",
                    tool.display()
                ));
            }
        }
    }

    open_cache_db(&command.cache_db_path).map_err(|e| e.to_string())?;

    let specs = all_lunar_specs();
    let selected: Vec<LunarSpec> = specs
        .iter()
        .filter(|s| command.zoom_buckets.contains(&s.zoom_bucket))
        .copied()
        .collect();

    if selected.is_empty() {
        return Err("No zoom buckets selected.".to_owned());
    }

    // ── Collect work ──────────────────────────────────────────────────────────
    progress(ContourBuildProgress::info(
        "Planning",
        0.0,
        "Scanning tiles…",
    ));

    struct TileWork {
        tile: TileKey,
        bounds: GeoBounds,
        spec: LunarSpec,
    }

    // SLDEM2015 covers ±60° latitude only
    const SLDEM_LAT_LIMIT: f32 = 60.0;
    let req_min_lat = command.min_lat.max(-SLDEM_LAT_LIMIT);
    let req_max_lat = command.max_lat.min(SLDEM_LAT_LIMIT);

    if req_min_lat >= req_max_lat {
        return Err(format!(
            "Requested bbox is entirely outside SLDEM coverage (±{SLDEM_LAT_LIMIT}°)."
        ));
    }

    let conn = open_cache_db(&command.cache_db_path).map_err(|e| e.to_string())?;
    let existing = crate::contours::load_manifest_set(&conn).unwrap_or_default();
    drop(conn);
    let mut work: Vec<TileWork> = Vec::new();
    let mut skipped = 0usize;

    for spec in &selected {
        let step = spec.half_extent_deg * 0.45;
        for lat_bucket in bucket_range(req_min_lat, req_max_lat, step) {
            let center_lat = (lat_bucket as f32 * step).clamp(-89.999, 89.999);
            // Skip tiles whose centre is outside SLDEM coverage
            if center_lat.abs() > SLDEM_LAT_LIMIT + spec.half_extent_deg {
                continue;
            }
            for lon_bucket in bucket_range(command.min_lon, command.max_lon, step) {
                let tile = TileKey {
                    zoom_bucket: spec.zoom_bucket,
                    lat_bucket,
                    lon_bucket,
                };
                if existing.contains(&(tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket)) {
                    skipped += 1;
                    continue;
                }
                let center_lon = lon_bucket as f32 * step;
                let bounds = GeoBounds {
                    min_lat: (center_lat - spec.half_extent_deg).clamp(-89.999, 89.999),
                    max_lat: (center_lat + spec.half_extent_deg).clamp(-89.999, 89.999),
                    min_lon: center_lon - spec.half_extent_deg,
                    max_lon: center_lon + spec.half_extent_deg,
                };
                work.push(TileWork {
                    tile,
                    bounds,
                    spec: *spec,
                });
            }
        }
    }

    let total = work.len() + skipped;
    let to_build = work.len();
    progress(ContourBuildProgress::info(
        "Planning",
        0.0,
        format!("{to_build} tiles to build, {skipped} already cached, {total} total"),
    ));
    progress(ContourBuildProgress::info(
        "Planning",
        0.0,
        format!(
            "Reusing persistent lunar source chunks in {}",
            source_chunk_root(&command.cache_db_path).display()
        ),
    ));

    if work.is_empty() {
        return Ok(format!(
            "Lunar contours complete: 0 built, {skipped} already cached."
        ));
    }

    {
        let mut by_zoom: BTreeMap<i32, Vec<_>> = BTreeMap::new();
        for item in work.drain(..) {
            by_zoom.entry(item.tile.zoom_bucket).or_default().push(item);
        }
        let buckets: Vec<Vec<_>> = by_zoom.into_values().collect();
        let max_len = buckets.iter().map(|v| v.len()).max().unwrap_or(0);
        for i in 0..max_len {
            for bucket in &buckets {
                if let Some(item) = bucket.get(i) {
                    work.push(TileWork {
                        tile: item.tile,
                        bounds: item.bounds,
                        spec: item.spec,
                    });
                }
            }
        }
    }

    let mut source_chunks: BTreeMap<PathBuf, SourceChunk> = BTreeMap::new();
    for item in &work {
        let chunk = source_chunk_for_bounds(&command.cache_db_path, item.bounds, item.spec);
        source_chunks.entry(chunk.path.clone()).or_insert(chunk);
    }
    let missing_chunks: Vec<SourceChunk> = source_chunks
        .values()
        .filter(|chunk| !chunk.path.exists())
        .cloned()
        .collect();

    progress(ContourBuildProgress::info(
        "Preparing",
        0.0,
        format!(
            "{} source chunks needed ({} already cached)",
            source_chunks.len(),
            source_chunks.len().saturating_sub(missing_chunks.len())
        ),
    ));

    for (index, chunk) in missing_chunks.iter().enumerate() {
        let fraction = index as f32 / missing_chunks.len().max(1) as f32;
        progress(ContourBuildProgress::info(
            "Preparing",
            fraction,
            format!(
                "[{}/{}] decoding SLDEM chunk {}",
                index + 1,
                missing_chunks.len(),
                chunk.path.display()
            ),
        ));
        ensure_source_chunk(
            &command.jp2_path,
            &command.cache_db_path,
            &gdal_translate,
            chunk.spec,
            chunk.bounds,
        )?;
    }

    progress(ContourBuildProgress::info(
        "Building",
        0.0,
        format!(
            "Native engine over cached chunks — {} tiles, {} source chunks",
            to_build,
            source_chunks.len()
        ),
    ));

    type ComputeResult = (TileKey, GeoBounds, Vec<(i64, f32, Vec<u8>)>);
    let (compute_tx, compute_rx) = mpsc::sync_channel::<ComputeResult>(32);
    enum Outcome {
        Built(GeoBounds),
        Error(String),
    }
    let (outcome_tx, rx) = mpsc::channel::<Outcome>();
    let done_count = Arc::new(AtomicUsize::new(0));
    let shared_chunks = SharedLunarChunkCache::new(command.cache_db_path.clone(), 8);

    std::thread::spawn(move || {
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .expect("rayon pool");
        pool.install(|| {
            work.into_par_iter()
                .map_init(
                    || {
                        LunarChunkSampler::new(
                            shared_chunks.clone(),
                            LunarSpec {
                                half_extent_deg: 0.55,
                                raster_size: 704,
                                interval_m: 50,
                                zoom_bucket: 4,
                            },
                        )
                    },
                    |sampler, item| {
                        sampler.spec = item.spec;
                        let (contours, _) = build_tile_contours_with_sampler(
                            focus_spec(item.spec),
                            item.bounds,
                            |lat, lon| sampler.sample(lat, lon),
                        );
                        let encoded: Vec<(i64, f32, Vec<u8>)> = contours
                            .into_iter()
                            .enumerate()
                            .map(|(fid, line)| {
                                (
                                    fid as i64,
                                    line.elevation_m,
                                    encode_gpkg_linestring(&line.points),
                                )
                            })
                            .collect();
                        (item.tile, item.bounds, encoded)
                    },
                )
                .for_each_with(compute_tx, |tx, result| {
                    let _ = tx.send(result);
                });
        });
    });

    let cache_db_owned = command.cache_db_path.clone();
    let done_arc = done_count.clone();
    std::thread::spawn(move || {
        let mut conn = match open_cache_db(&cache_db_owned) {
            Ok(conn) => conn,
            Err(e) => {
                let _ = outcome_tx.send(Outcome::Error(format!("open cache DB failed: {e}")));
                return;
            }
        };
        for (tile, tile_bounds, encoded) in compute_rx {
            let outcome = write_tile_native_lunar_encoded(&mut conn, tile, &encoded);
            done_arc.fetch_add(1, Ordering::Relaxed);
            let _ = outcome_tx.send(match outcome {
                Ok(()) => Outcome::Built(tile_bounds),
                Err(e) => Outcome::Error(format!(
                    "z{} ({},{}) — {e}",
                    tile.zoom_bucket, tile.lat_bucket, tile.lon_bucket
                )),
            });
        }
    });

    let mut built = 0usize;
    let mut errors = 0usize;
    for outcome in rx {
        let done = done_count.load(Ordering::Relaxed);
        let fraction = done as f32 / to_build.max(1) as f32;
        match outcome {
            Outcome::Built(tile_bounds) => {
                built += 1;
                progress(ContourBuildProgress::built(
                    "Building",
                    fraction,
                    format!("{done}/{to_build} tiles built"),
                    (
                        tile_bounds.min_lat,
                        tile_bounds.max_lat,
                        tile_bounds.min_lon,
                        tile_bounds.max_lon,
                    ),
                ));
            }
            Outcome::Error(message) => {
                errors += 1;
                progress(ContourBuildProgress::error("Building", fraction, message));
            }
        }
    }

    if let Ok(conn) = Connection::open(&command.cache_db_path) {
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }

    Ok(format!(
        "Lunar contours complete: {built} built, {skipped} already cached, {errors} errors, {total} total tiles"
    ))
}
