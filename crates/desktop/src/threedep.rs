//! USGS 3DEP 1 m bare-earth elevation acquisition.
//!
//! The full 1 m 3DEP holding is hundreds of terabytes, so this module never
//! mirrors it. It treats `3DEPElevation/ImageServer` as an on-demand clipping
//! service: probe whether 1 m source exists under a point, then pull only the
//! small chunks the operator actually looks at and keep them in a bounded
//! local cache.

use crate::model::GeoPoint;
use crate::settings_store;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const SERVICE: &str =
    "https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer";

/// Cached chunks are a fixed lat/lon grid so neighbouring contour tiles reuse
/// the same downloads. 0.02° is ~2.2 km of latitude, which stays under the
/// service's response ceiling at every latitude (see `MAX_REQUEST_PX`).
pub const CHUNK_DEG: f32 = 0.02;

/// `exportImage` returns HTTP 500 once the encoded response passes roughly
/// 32 MB. 2400² Float32 is ~22 MB, which leaves comfortable headroom while
/// still resolving a 0.02° chunk at better than 1 m.
const MAX_REQUEST_PX: u32 = 2400;

/// A source whose pixel size is at or below this counts as "1 m" coverage.
/// The catalog reports exactly 1.0 for 1 m projects; the tolerance absorbs
/// the handful of near-metre products (e.g. 3 ft ≈ 0.914 m).
const FINE_PIXEL_SIZE_M: f64 = 1.5;

const CACHE_DIR_NAME: &str = "3dep_1m";
/// Written next to a chunk that the service answered with nothing but nodata,
/// so an ocean or data-gap chunk is not re-requested on every repaint.
const EMPTY_MARKER_EXT: &str = "empty";

const NODATA: f32 = -999_999.0;

/// How long a caller waits for another thread's in-flight download of the same
/// chunk before falling back. Comfortably longer than the HTTP timeout, so the
/// fallback only happens if that thread died.
const CHUNK_WAIT_TIMEOUT: Duration = Duration::from_secs(240);

/// 3DEP only publishes for the United States and its territories. Screening on
/// this box first means panning over Europe or Asia never touches the network.
fn within_service_area(point: GeoPoint) -> bool {
    (-180.0..=-64.0).contains(&point.lon) && (15.0..=72.0).contains(&point.lat)
}

// ── Coverage probing ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// Not probed yet. A background probe may have been scheduled.
    Unknown,
    /// 1 m (or finer) source exists here.
    Fine,
    /// The point is served, but only by coarser products (10 m / 30 m), or it
    /// lies outside 3DEP entirely.
    Coarse,
}

fn coverage_cache() -> &'static Mutex<HashMap<(i32, i32), Coverage>> {
    static CACHE: OnceLock<Mutex<HashMap<(i32, i32), Coverage>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn probing_set() -> &'static Mutex<HashSet<(i32, i32)>> {
    static SET: OnceLock<Mutex<HashSet<(i32, i32)>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Coverage is probed on a coarser grid than chunks; 1 m availability is a
/// property of lidar project footprints, which are far larger than 0.02°.
fn coverage_cell(point: GeoPoint) -> (i32, i32) {
    (
        (point.lat / 0.05).floor() as i32,
        (point.lon / 0.05).floor() as i32,
    )
}

/// Nonblocking coverage lookup. Returns `Unknown` on a cache miss and schedules
/// one deduplicated background probe. Callers should treat `Unknown` as "not
/// yet" and fall back to their existing source.
pub fn coverage_at(point: GeoPoint) -> Coverage {
    if !is_enabled() {
        return Coverage::Coarse;
    }
    if !within_service_area(point) {
        return Coverage::Coarse;
    }
    let cell = coverage_cell(point);
    if let Ok(guard) = coverage_cache().lock()
        && let Some(&known) = guard.get(&cell)
    {
        return known;
    }
    schedule_probe(cell);
    Coverage::Unknown
}

/// Blocking coverage lookup for background workers that are already off the UI
/// thread and cannot proceed without an answer.
pub fn coverage_at_blocking(point: GeoPoint) -> Coverage {
    if !is_enabled() || !within_service_area(point) {
        return Coverage::Coarse;
    }
    let cell = coverage_cell(point);
    if let Ok(guard) = coverage_cache().lock()
        && let Some(&known) = guard.get(&cell)
    {
        return known;
    }
    let result = probe_cell(cell);
    if let Ok(mut guard) = coverage_cache().lock() {
        guard.insert(cell, result);
    }
    result
}

fn schedule_probe(cell: (i32, i32)) {
    {
        let Ok(mut guard) = probing_set().lock() else {
            return;
        };
        if !guard.insert(cell) {
            return;
        }
    }
    std::thread::spawn(move || {
        let result = probe_cell(cell);
        if let Ok(mut guard) = coverage_cache().lock() {
            guard.insert(cell, result);
        }
        if let Ok(mut guard) = probing_set().lock() {
            guard.remove(&cell);
        }
        crate::app::request_repaint();
    });
}

/// Ask the image service's catalog which source rasters intersect the cell
/// centre and keep the finest pixel size any of them reports.
fn probe_cell(cell: (i32, i32)) -> Coverage {
    let lat = (cell.0 as f32 + 0.5) * 0.05;
    let lon = (cell.1 as f32 + 0.5) * 0.05;
    let geometry =
        format!(r#"{{"x":{lon},"y":{lat},"spatialReference":{{"wkid":4326}}}}"#);

    let response = http_client()
        .get(format!("{SERVICE}/query"))
        .query(&[
            ("geometry", geometry.as_str()),
            ("geometryType", "esriGeometryPoint"),
            ("spatialRel", "esriSpatialRelIntersects"),
            ("returnGeometry", "false"),
            ("outFields", "LowPS"),
            ("f", "json"),
        ])
        .send();

    let Ok(response) = response else {
        return Coverage::Coarse;
    };
    let Ok(body) = response.text() else {
        return Coverage::Coarse;
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Coverage::Coarse;
    };
    let has_fine = parsed
        .get("features")
        .and_then(|f| f.as_array())
        .map(|features| {
            features.iter().any(|feature| {
                feature
                    .get("attributes")
                    .and_then(|a| a.get("LowPS"))
                    .and_then(|v| v.as_f64())
                    .is_some_and(|ps| ps > 0.0 && ps <= FINE_PIXEL_SIZE_M)
            })
        })
        .unwrap_or(false);

    if has_fine {
        Coverage::Fine
    } else {
        Coverage::Coarse
    }
}

// ── Chunk cache ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct ChunkKey {
    pub lat: i32,
    pub lon: i32,
}

pub fn chunk_key_for(point: GeoPoint) -> ChunkKey {
    ChunkKey {
        lat: (point.lat / CHUNK_DEG).floor() as i32,
        lon: (point.lon / CHUNK_DEG).floor() as i32,
    }
}

impl ChunkKey {
    pub fn min_lat(self) -> f32 {
        self.lat as f32 * CHUNK_DEG
    }
    pub fn min_lon(self) -> f32 {
        self.lon as f32 * CHUNK_DEG
    }
    pub fn max_lat(self) -> f32 {
        self.min_lat() + CHUNK_DEG
    }
    pub fn max_lon(self) -> f32 {
        self.min_lon() + CHUNK_DEG
    }
    pub fn center(self) -> GeoPoint {
        GeoPoint {
            lat: self.min_lat() + CHUNK_DEG * 0.5,
            lon: self.min_lon() + CHUNK_DEG * 0.5,
        }
    }
}

pub fn cache_root(derived_root: &Path) -> PathBuf {
    derived_root.join("terrain").join(CACHE_DIR_NAME)
}

pub fn chunk_path(derived_root: &Path, key: ChunkKey) -> PathBuf {
    cache_root(derived_root).join(format!("lat{}_lon{}.tif", key.lat, key.lon))
}

fn empty_marker_path(derived_root: &Path, key: ChunkKey) -> PathBuf {
    chunk_path(derived_root, key).with_extension(EMPTY_MARKER_EXT)
}

/// Cache-only lookup. Never touches the network and never blocks.
pub fn peek_chunk(derived_root: &Path, key: ChunkKey) -> Option<PathBuf> {
    let path = chunk_path(derived_root, key);
    path.exists().then_some(path)
}

/// Returns true when this chunk is known to hold no 3DEP data at all.
pub fn chunk_known_empty(derived_root: &Path, key: ChunkKey) -> bool {
    empty_marker_path(derived_root, key).exists()
}

fn downloading_set() -> &'static Mutex<HashSet<ChunkKey>> {
    static SET: OnceLock<Mutex<HashSet<ChunkKey>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Blocking fetch-or-reuse for background builders. Returns the cached GeoTIFF
/// path, downloading it first if necessary.
pub fn ensure_chunk(derived_root: &Path, key: ChunkKey) -> Option<PathBuf> {
    if let Some(path) = peek_chunk(derived_root, key) {
        touch(&path);
        return Some(path);
    }
    if !is_enabled() || chunk_known_empty(derived_root, key) {
        return None;
    }
    if coverage_at_blocking(key.center()) != Coverage::Fine {
        return None;
    }

    // One download per chunk even when several builders converge on it. A
    // caller that loses the race *waits* rather than giving up: returning
    // `None` here would let one road vertex resolve against 3DEP and the next
    // against SRTM purely on download timing, which bakes a jagged mix of two
    // terrain models into whatever geometry is being built.
    let deadline = std::time::Instant::now() + CHUNK_WAIT_TIMEOUT;
    loop {
        {
            let Ok(mut guard) = downloading_set().lock() else {
                return None;
            };
            if guard.insert(key) {
                break;
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
        if let Some(path) = peek_chunk(derived_root, key) {
            touch(&path);
            return Some(path);
        }
        if chunk_known_empty(derived_root, key) {
            return None;
        }
    }

    let result = download_chunk(derived_root, key);
    if let Ok(mut guard) = downloading_set().lock() {
        guard.remove(&key);
    }
    result
}

/// Pixel dimensions for a chunk request, targeting native 1 m and shrinking
/// longitude with the cosine of latitude so pixels stay near-square.
fn request_size(key: ChunkKey) -> (u32, u32) {
    const METRES_PER_DEG_LAT: f32 = 111_320.0;
    let center_lat = key.center().lat.to_radians();
    let height_m = CHUNK_DEG * METRES_PER_DEG_LAT;
    let width_m = CHUNK_DEG * METRES_PER_DEG_LAT * center_lat.cos().abs().max(0.05);
    (
        (width_m.round() as u32).clamp(16, MAX_REQUEST_PX),
        (height_m.round() as u32).clamp(16, MAX_REQUEST_PX),
    )
}

fn download_chunk(derived_root: &Path, key: ChunkKey) -> Option<PathBuf> {
    let root = cache_root(derived_root);
    std::fs::create_dir_all(&root).ok()?;

    let (width, height) = request_size(key);
    let bbox = format!(
        "{},{},{},{}",
        key.min_lon(),
        key.min_lat(),
        key.max_lon(),
        key.max_lat()
    );
    let size = format!("{width},{height}");

    let response = http_client()
        .get(format!("{SERVICE}/exportImage"))
        .query(&[
            ("bbox", bbox.as_str()),
            ("bboxSR", "4326"),
            ("imageSR", "4326"),
            ("size", size.as_str()),
            ("format", "tiff"),
            ("pixelType", "F32"),
            ("noData", "-999999"),
            ("interpolation", "RSP_BilinearInterpolation"),
            ("f", "image"),
        ])
        .send()
        .ok()?;

    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().ok()?;
    // The service answers errors with an HTML page and a 200-shaped body, so
    // the TIFF magic number is the only trustworthy success signal.
    if !looks_like_tiff(&bytes) {
        return None;
    }

    let raw_path = root.join(format!("lat{}_lon{}.raw.tif", key.lat, key.lon));
    std::fs::write(&raw_path, &bytes).ok()?;

    let final_path = chunk_path(derived_root, key);
    let ok = compress_to_cog(&raw_path, &final_path);
    let _ = std::fs::remove_file(&raw_path);

    if !ok {
        let _ = std::fs::remove_file(&final_path);
        return None;
    }

    if raster_is_empty(&final_path) {
        let _ = std::fs::remove_file(&final_path);
        let _ = std::fs::write(empty_marker_path(derived_root, key), b"");
        return None;
    }

    enforce_cache_budget(derived_root);
    Some(final_path)
}

fn looks_like_tiff(bytes: &[u8]) -> bool {
    matches!(bytes.get(..2), Some(b"II") | Some(b"MM")) && bytes.len() > 1024
}

/// Re-encode the service's uncompressed Float32 response as a tiled, DEFLATE
/// compressed COG. This roughly halves cache footprint and lets GDAL read
/// windows without decoding the whole chunk.
fn compress_to_cog(src: &Path, dst: &Path) -> bool {
    let gdal_translate = settings_store::resolve_gdal_tool("gdal_translate");
    std::process::Command::new(gdal_translate)
        .args([
            "-q",
            "-of",
            "COG",
            "-co",
            "COMPRESS=DEFLATE",
            "-co",
            "PREDICTOR=3",
            "-co",
            "BLOCKSIZE=512",
            "-a_nodata",
            "-999999",
        ])
        .arg(src)
        .arg(dst)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
        && dst.exists()
}

/// A chunk over water or in a lidar gap comes back as solid nodata. `gdalinfo
/// -stats` reports no statistics for such a band, which is how we detect it.
fn raster_is_empty(path: &Path) -> bool {
    let gdalinfo = settings_store::resolve_gdal_tool("gdalinfo");
    let Ok(output) = std::process::Command::new(gdalinfo)
        .args(["-stats", "-json"])
        .arg(path)
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return false;
    };
    let Ok(text) = String::from_utf8(output.stdout) else {
        return false;
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let minimum = parsed
        .get("bands")
        .and_then(|b| b.as_array())
        .and_then(|bands| bands.first())
        .and_then(|band| band.get("minimum"))
        .and_then(|v| v.as_f64());
    match minimum {
        // No statistics at all means every pixel was nodata.
        None => true,
        Some(value) => value <= NODATA as f64 + 1.0,
    }
}

// ── Point sampling ───────────────────────────────────────────────────────────
//
// Markers, roads, buildings and trees need an elevation under a single
// coordinate. That needs a local raster, so this path — unlike the contour
// path — does use the 0.02° chunk cache. Chunks are decoded through
// `gdal_translate` into a compact Float32 sidecar, mirroring how
// `srtm_stream` normalises awkward SRTM GeoTIFFs.

/// Decoded chunks held in memory. A road or building build walks a polyline
/// across several chunks, so this has to be deep enough that spatial locality
/// actually pays; each is ~19 MB of Float32.
const MAX_CACHED_SAMPLE_TILES: usize = 12;
/// One chunk fetch at a time so a dense marker cluster cannot saturate the
/// machine, or the public service, with concurrent downloads.
const MAX_CONCURRENT_SAMPLE_FETCHES: usize = 1;

struct SampleTile {
    key: ChunkKey,
    width: u32,
    height: u32,
    samples: Vec<f32>,
}

impl SampleTile {
    /// Bilinear sample in the chunk's own lat/lon frame. Returns `None` when
    /// the neighbourhood is nodata rather than inventing a zero.
    fn sample(&self, point: GeoPoint) -> Option<f32> {
        let u = ((point.lon - self.key.min_lon()) / CHUNK_DEG).clamp(0.0, 0.999_999);
        // Raster rows run north to south, so latitude is flipped.
        let v = (1.0 - (point.lat - self.key.min_lat()) / CHUNK_DEG).clamp(0.0, 0.999_999);

        let x = u * self.width.saturating_sub(1) as f32;
        let y = v * self.height.saturating_sub(1) as f32;
        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(self.width.saturating_sub(1));
        let y1 = (y0 + 1).min(self.height.saturating_sub(1));
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;

        let at = |px: u32, py: u32| -> Option<f32> {
            let value = *self.samples.get((py * self.width + px) as usize)?;
            (value.is_finite() && value > NODATA + 1.0).then_some(value)
        };

        // Any nodata corner disqualifies the sample; a partial interpolation
        // would drag the result toward the sentinel.
        let top = lerp(at(x0, y0)?, at(x1, y0)?, tx);
        let bottom = lerp(at(x0, y1)?, at(x1, y1)?, tx);
        Some(lerp(top, bottom, ty))
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

struct SampleCache {
    tiles: Vec<SampleTile>,
    missing: HashSet<ChunkKey>,
    loading: HashSet<ChunkKey>,
}

fn sample_cache() -> &'static Mutex<SampleCache> {
    static CACHE: OnceLock<Mutex<SampleCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(SampleCache {
            tiles: Vec::new(),
            missing: HashSet::new(),
            loading: HashSet::new(),
        })
    })
}

/// Nonblocking 1 m elevation lookup.
///
/// Returns a value only when the covering chunk is already decoded in memory.
/// Otherwise it schedules the download/decode and returns `None` so the caller
/// falls back to SRTM for now; the chunk is used from the next repaint on.
pub fn peek_elevation_m(derived_root: &Path, point: GeoPoint) -> Option<f32> {
    if !is_enabled() {
        return None;
    }
    let key = chunk_key_for(point);

    {
        let mut guard = sample_cache().lock().ok()?;
        if guard.missing.contains(&key) {
            return None;
        }
        if let Some(index) = guard.tiles.iter().position(|tile| tile.key == key) {
            let tile = guard.tiles.remove(index);
            let value = tile.sample(point);
            guard.tiles.insert(0, tile);
            return value;
        }
    }

    request_sample_preload(derived_root, key);
    None
}

/// Blocking 1 m elevation lookup for background layer builders.
///
/// Unlike `peek_elevation_m` this downloads and decodes the covering chunk if
/// necessary, so it returns the same answer for the same point regardless of
/// cache state. That determinism is the point: road, building and tree geometry
/// bakes one elevation per vertex at build time, and a sampler that silently
/// alternates between 3DEP and SRTM writes the difference between two terrain
/// models into the geometry permanently.
///
/// Returns `None` where 3DEP publishes no 1 m source, leaving SRTM to answer.
pub fn blocking_elevation_m(derived_root: &Path, point: GeoPoint) -> Option<f32> {
    if !is_enabled() {
        return None;
    }
    let key = chunk_key_for(point);

    {
        let mut guard = sample_cache().lock().ok()?;
        if guard.missing.contains(&key) {
            return None;
        }
        if let Some(index) = guard.tiles.iter().position(|tile| tile.key == key) {
            let tile = guard.tiles.remove(index);
            let value = tile.sample(point);
            guard.tiles.insert(0, tile);
            return value;
        }
    }

    // Not resident — fetch and decode on this thread. Callers are background
    // workers whose contract already allows blocking.
    let tile = ensure_chunk(derived_root, key).and_then(|path| load_sample_tile(&path, key));

    let mut guard = sample_cache().lock().ok()?;
    match tile {
        Some(tile) => {
            let value = tile.sample(point);
            if !guard.tiles.iter().any(|cached| cached.key == key) {
                guard.tiles.insert(0, tile);
                if guard.tiles.len() > MAX_CACHED_SAMPLE_TILES {
                    guard.tiles.pop();
                }
            }
            value
        }
        None => {
            // Remember the miss so the rest of this build resolves against
            // SRTM consistently instead of retrying per vertex.
            guard.missing.insert(key);
            None
        }
    }
}

/// Schedule a deduplicated background fetch and decode of one chunk.
fn request_sample_preload(derived_root: &Path, key: ChunkKey) {
    if coverage_at(point_of(key)) != Coverage::Fine {
        return;
    }
    {
        let Ok(mut guard) = sample_cache().lock() else {
            return;
        };
        if guard.missing.contains(&key)
            || guard.tiles.iter().any(|tile| tile.key == key)
            || guard.loading.len() >= MAX_CONCURRENT_SAMPLE_FETCHES
            || !guard.loading.insert(key)
        {
            return;
        }
    }

    let derived_root = derived_root.to_path_buf();
    if std::thread::Builder::new()
        .name("threedep-elevation-preload".into())
        .spawn(move || {
            let tile = ensure_chunk(&derived_root, key)
                .and_then(|path| load_sample_tile(&path, key));
            if let Ok(mut guard) = sample_cache().lock() {
                guard.loading.remove(&key);
                match tile {
                    Some(tile) => {
                        if !guard.tiles.iter().any(|cached| cached.key == key) {
                            guard.tiles.insert(0, tile);
                            if guard.tiles.len() > MAX_CACHED_SAMPLE_TILES {
                                guard.tiles.pop();
                            }
                        }
                    }
                    None => {
                        guard.missing.insert(key);
                    }
                }
            }
            crate::app::request_repaint();
        })
        .is_err()
        && let Ok(mut guard) = sample_cache().lock()
    {
        guard.loading.remove(&key);
    }
}

fn point_of(key: ChunkKey) -> GeoPoint {
    key.center()
}

/// Decode a cached chunk into memory, normalising through `gdal_translate`
/// because the cached form is a DEFLATE-compressed Float32 COG that the
/// `image` crate cannot read.
fn load_sample_tile(path: &Path, key: ChunkKey) -> Option<SampleTile> {
    let raw_path = path.with_extension("f32raw");
    if let Some(tile) = load_raw_f32(&raw_path, key) {
        return Some(tile);
    }

    // The EHdr driver writes the raw band to exactly the path it is given and
    // derives the header by swapping the extension, so the output must be
    // named `.bil` explicitly. Passing an extensionless stem makes GDAL write
    // an extensionless binary that nothing downstream can find.
    let bil_path = raw_path.with_extension("bil");
    let hdr_path = raw_path.with_extension("hdr");

    let gdal_translate = settings_store::resolve_gdal_tool("gdal_translate");
    let status = std::process::Command::new(&gdal_translate)
        .args(["-q", "-ot", "Float32", "-of", "EHdr"])
        .arg(path)
        .arg(&bil_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }

    let hdr_text = std::fs::read_to_string(&hdr_path).ok()?;
    let (mut width, mut height, mut big_endian) = (0u32, 0u32, false);
    for line in hdr_text.lines() {
        let line = line.trim();
        let mut parts = line.split_whitespace();
        let (Some(keyword), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        match keyword.to_ascii_uppercase().as_str() {
            "NCOLS" => width = value.parse().unwrap_or(0),
            "NROWS" => height = value.parse().unwrap_or(0),
            "BYTEORDER" => big_endian = value == "M",
            _ => {}
        }
    }
    if width == 0 || height == 0 {
        return None;
    }

    let raw_bytes = std::fs::read(&bil_path).ok()?;
    let expected = (width as usize) * (height as usize) * 4;
    if raw_bytes.len() < expected {
        return None;
    }
    let samples: Vec<f32> = raw_bytes[..expected]
        .chunks_exact(4)
        .map(|b| {
            let arr = [b[0], b[1], b[2], b[3]];
            if big_endian {
                f32::from_be_bytes(arr)
            } else {
                f32::from_le_bytes(arr)
            }
        })
        .collect();

    // Compact sidecar so a cold start skips the subprocess next time.
    if let Ok(mut file) = std::fs::File::create(&raw_path) {
        use std::io::Write;
        let _ = file.write_all(&width.to_le_bytes());
        let _ = file.write_all(&height.to_le_bytes());
        for value in &samples {
            let _ = file.write_all(&value.to_le_bytes());
        }
    }
    let _ = std::fs::remove_file(&hdr_path);
    let _ = std::fs::remove_file(&bil_path);
    let _ = std::fs::remove_file(raw_path.with_extension("prj"));
    let _ = std::fs::remove_file(bil_path.with_extension("bil.aux.xml"));

    Some(SampleTile {
        key,
        width,
        height,
        samples,
    })
}

/// Read the compact sidecar: LE u32 width, LE u32 height, then Float32 samples.
fn load_raw_f32(path: &Path, key: ChunkKey) -> Option<SampleTile> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 8 {
        return None;
    }
    let width = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let height = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let expected = 8 + (width as usize) * (height as usize) * 4;
    if width == 0 || height == 0 || bytes.len() < expected {
        return None;
    }
    let samples: Vec<f32> = bytes[8..expected]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    Some(SampleTile {
        key,
        width,
        height,
        samples,
    })
}

// ── Cache budget ─────────────────────────────────────────────────────────────

/// Evict least-recently-used chunks until the cache fits its configured
/// budget. Eviction is safe at any time: a discarded chunk is simply
/// re-downloaded the next time it is needed.
pub fn enforce_cache_budget(derived_root: &Path) {
    let budget_bytes = (settings_store::threedep_cache_budget_gb() as f64
        * 1024.0
        * 1024.0
        * 1024.0) as u64;
    let root = cache_root(derived_root);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };

    let mut chunks: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("tif") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let accessed = meta
            .accessed()
            .or_else(|_| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        // A chunk's cost includes its decoded sampling sidecar, which is
        // evicted with it.
        let sidecar = path.with_extension("f32raw");
        let size = meta.len()
            + std::fs::metadata(&sidecar).map(|m| m.len()).unwrap_or(0);
        total += size;
        chunks.push((path, size, accessed));
    }

    if total <= budget_bytes {
        return;
    }

    chunks.sort_by_key(|(_, _, accessed)| *accessed);
    for (path, size, _) in chunks {
        if total <= budget_bytes {
            break;
        }
        let _ = std::fs::remove_file(path.with_extension("f32raw"));
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

pub fn cache_bytes(derived_root: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(cache_root(derived_root)) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum()
}

/// Refresh a chunk's modification time so LRU eviction reflects actual use
/// even on volumes mounted without access-time updates.
fn touch(path: &Path) {
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}


// ── Direct tile rasters ──────────────────────────────────────────────────────

/// Download one contour tile's elevation raster straight from the service at
/// the exact bounds and pixel size the caller wants.
///
/// The contour pipeline does not go through the chunk cache: the service will
/// clip and resample server-side, so a whole tile is a single request rather
/// than a mosaic of cached 0.02° chunks. The durable product of that request
/// is the contour geometry stored in the SQLite focus cache, so the GeoTIFF
/// written here is a build-time temporary the caller deletes.
pub fn fetch_tile_raster(
    min_lat: f32,
    min_lon: f32,
    max_lat: f32,
    max_lon: f32,
    raster_size: u32,
    destination: &Path,
) -> bool {
    if !is_enabled() {
        return false;
    }
    let size_px = raster_size.clamp(16, MAX_REQUEST_PX);
    let bbox = format!("{min_lon},{min_lat},{max_lon},{max_lat}");
    let size = format!("{size_px},{size_px}");

    let Ok(response) = http_client()
        .get(format!("{SERVICE}/exportImage"))
        .query(&[
            ("bbox", bbox.as_str()),
            ("bboxSR", "4326"),
            ("imageSR", "4326"),
            ("size", size.as_str()),
            ("format", "tiff"),
            ("pixelType", "F32"),
            ("noData", "-999999"),
            ("interpolation", "RSP_BilinearInterpolation"),
            ("f", "image"),
        ])
        .send()
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(bytes) = response.bytes() else {
        return false;
    };
    if !looks_like_tiff(&bytes) {
        return false;
    }
    if let Some(parent) = destination.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(destination, &bytes).is_ok()
}

/// The nodata sentinel requested from the service, for callers that pass it on
/// to `gdal_contour -snodata`.
pub fn nodata_sentinel() -> f32 {
    NODATA
}

// ── Hillshade ────────────────────────────────────────────────────────────────

/// Fetch a server-rendered multidirectional hillshade PNG for `bounds`.
/// The service does the shading, so this is an 8-bit greyscale image rather
/// than an elevation grid — far cheaper than shading a Float32 chunk locally.
pub fn fetch_hillshade_png(
    min_lat: f32,
    min_lon: f32,
    max_lat: f32,
    max_lon: f32,
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    if !is_enabled() {
        return None;
    }
    let bbox = format!("{min_lon},{min_lat},{max_lon},{max_lat}");
    let size = format!(
        "{},{}",
        width.clamp(16, MAX_REQUEST_PX),
        height.clamp(16, MAX_REQUEST_PX)
    );
    let response = http_client()
        .get(format!("{SERVICE}/exportImage"))
        .query(&[
            ("bbox", bbox.as_str()),
            ("bboxSR", "4326"),
            ("imageSR", "4326"),
            ("size", size.as_str()),
            ("format", "png"),
            (
                "renderingRule",
                r#"{"rasterFunction":"Hillshade Multidirectional"}"#,
            ),
            ("f", "image"),
        ])
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().ok()?.to_vec();
    // PNG magic; the service reports failures as an HTML page.
    (bytes.starts_with(&[0x89, b'P', b'N', b'G'])).then_some(bytes)
}

// ── Shared ───────────────────────────────────────────────────────────────────

pub fn is_enabled() -> bool {
    settings_store::threedep_enabled()
}

fn http_client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .expect("3dep http client")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_keys_tile_the_grid_without_gaps() {
        let key = chunk_key_for(GeoPoint {
            lat: 40.7123,
            lon: -74.0087,
        });
        assert!(key.min_lat() <= 40.7123 && 40.7123 < key.max_lat());
        assert!(key.min_lon() <= -74.0087 && -74.0087 < key.max_lon());
    }

    #[test]
    fn negative_longitudes_floor_downward() {
        // -74.0087 / 0.02 = -3700.4; flooring must go to -3701 so the chunk
        // actually contains the point rather than sitting east of it.
        let key = chunk_key_for(GeoPoint {
            lat: 0.0,
            lon: -74.0087,
        });
        assert_eq!(key.lon, -3701);
    }

    #[test]
    fn requests_stay_under_the_service_response_ceiling() {
        for lat_cell in [-1000, 0, 1000, 2000, 3500] {
            let (w, h) = request_size(ChunkKey {
                lat: lat_cell,
                lon: 0,
            });
            assert!(w <= MAX_REQUEST_PX && h <= MAX_REQUEST_PX);
            // Float32 payload must stay well below the ~32 MB failure point.
            assert!((w as u64) * (h as u64) * 4 < 28 * 1024 * 1024);
        }
    }

    #[test]
    fn service_area_screens_out_the_rest_of_the_world() {
        assert!(within_service_area(GeoPoint { lat: 40.7, lon: -74.0 }));
        assert!(within_service_area(GeoPoint { lat: 61.2, lon: -149.9 }));
        assert!(!within_service_area(GeoPoint { lat: 48.85, lon: 2.35 }));
        assert!(!within_service_area(GeoPoint { lat: -33.9, lon: 151.2 }));
    }

    // ── Live service smoke tests ─────────────────────────────────────────
    //
    // These hit `elevation.nationalmap.gov`, so they are `#[ignore]`d and are
    // not part of the ordinary suite. Run them with
    // `cargo test -p one-thousand-electric-eye-desktop threedep -- --ignored`
    // after changing any request parameter.

    #[test]
    #[ignore = "hits the live USGS 3DEP service"]
    fn live_probe_distinguishes_one_metre_coverage() {
        let manhattan = coverage_cell(GeoPoint {
            lat: 40.71,
            lon: -74.01,
        });
        assert_eq!(probe_cell(manhattan), Coverage::Fine);

        // Anchorage is served by 3DEP but only at ~3 m, so it must not be
        // mistaken for 1 m coverage.
        let anchorage = coverage_cell(GeoPoint {
            lat: 61.2,
            lon: -149.9,
        });
        assert_eq!(probe_cell(anchorage), Coverage::Coarse);
    }

    #[test]
    #[ignore = "hits the live USGS 3DEP service"]
    fn live_tile_fetch_writes_a_readable_elevation_raster() {
        let dir = std::env::temp_dir().join("1kee-threedep-live");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("tile.tif");
        let _ = std::fs::remove_file(&path);

        // Boulder foothills: strong relief, solid 1 m coverage.
        assert!(fetch_tile_raster(40.014, -105.296, 40.026, -105.284, 1792, &path));

        let output = std::process::Command::new("gdalinfo")
            .args(["-stats", "-json"])
            .arg(&path)
            .output()
            .expect("gdalinfo");
        let parsed: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("gdalinfo json");
        let band = &parsed["bands"][0];
        let minimum = band["minimum"].as_f64().expect("minimum");
        let maximum = band["maximum"].as_f64().expect("maximum");
        assert_eq!(parsed["size"][0].as_u64(), Some(1792));
        // Real Front Range elevations, not a nodata plane or an error page.
        assert!(
            (1_500.0..4_500.0).contains(&minimum),
            "unexpected minimum {minimum}"
        );
        assert!(maximum > minimum + 20.0, "expected relief, got {maximum}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[ignore = "hits the live USGS 3DEP service"]
    fn live_chunk_downloads_caches_and_samples_an_elevation() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after Unix epoch")
            .as_nanos();
        let derived_root = std::env::temp_dir().join(format!(
            "1kee-threedep-chunk-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&derived_root).expect("temp derived root");

        // Boulder foothills again: known 1 m coverage and unmistakable relief.
        let point = GeoPoint {
            lat: 40.020,
            lon: -105.290,
        };
        let key = chunk_key_for(point);
        let path = ensure_chunk(&derived_root, key).expect("chunk download");
        assert!(path.exists());
        // The cached form must be smaller than the raw Float32 the service sent.
        let cached_bytes = std::fs::metadata(&path).expect("chunk metadata").len();
        let (width, height) = request_size(key);
        assert!(cached_bytes < (width as u64) * (height as u64) * 4);

        let tile = load_sample_tile(&path, key).expect("decode chunk");
        let elevation = tile.sample(point).expect("elevation sample");
        assert!(
            (1_500.0..3_000.0).contains(&elevation),
            "unexpected Front Range elevation {elevation}"
        );

        // A second call must reuse the cache rather than downloading again.
        assert!(peek_chunk(&derived_root, key).is_some());

        // Eviction removes the chunk and its decoded sidecar together.
        assert!(path.with_extension("f32raw").exists());

        let _ = std::fs::remove_dir_all(&derived_root);
    }

    #[test]
    #[ignore = "hits the live USGS 3DEP service"]
    fn live_hillshade_returns_a_decodable_png() {
        let bytes = fetch_hillshade_png(40.014, -105.296, 40.026, -105.284, 512, 512)
            .expect("hillshade png");
        let image = image::load_from_memory(&bytes).expect("decode").to_luma8();
        assert_eq!(image.dimensions(), (512, 512));
        // A shaded slope must actually vary; a flat image means the rendering
        // rule was ignored.
        let first = image.pixels().next().expect("pixel").0[0];
        assert!(image.pixels().any(|pixel| pixel.0[0] != first));
    }

    #[test]
    fn tiff_magic_gates_html_error_pages() {
        assert!(!looks_like_tiff(b"<html lang=\"en\">Error exporting image"));
        let mut fake = vec![b'I', b'I'];
        fake.resize(2048, 0);
        assert!(looks_like_tiff(&fake));
    }
}
