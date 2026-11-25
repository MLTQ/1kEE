/// Pure-Rust contour extraction using marching squares.
///
/// Reads SRTM elevation tiles, builds a raster grid at the tile spec's
/// resolution, and extracts iso-lines at every `interval_m` multiple.
/// Produces the same output schema as the GDAL pipeline so the desktop
/// reads both transparently.
use crate::contours::{FocusContourSpec, GeoBounds};
use hashlink::LruCache;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Fast deterministic non-cryptographic hasher for `u64` endpoint keys.
#[derive(Default)]
pub(crate) struct FxHasher(u64);

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x517cc1b727220a95);
        }
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517cc1b727220a95);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type FxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

// ── SRTM tile reader (NaN for nodata / missing tiles) ─────────────────────────

pub(crate) enum SrtmSamples {
    /// Compact 2 bytes/pixel storage for canonical signed Int16 SRTM (`i16::MIN` = nodata).
    I16(Vec<i16>),
    /// Float32 storage for reprojected products (`NaN` = nodata).
    F32(Vec<f32>),
}

pub(crate) struct SrtmTile {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) samples: SrtmSamples,
}

impl SrtmTile {
    #[cfg(test)]
    #[inline]
    pub(crate) fn sample_at_index(&self, idx: usize) -> f32 {
        match &self.samples {
            SrtmSamples::I16(v) => {
                let s = v[idx];
                if s <= -32767 { f32::NAN } else { s as f32 }
            }
            SrtmSamples::F32(v) => v[idx],
        }
    }
    /// Load an SRTM GeoTIFF tile.  Handles both signed Int16 (the most common
    /// SRTM distribution format) and Float32 (some re-processed products).
    pub(crate) fn load(path: &Path) -> Option<Self> {
        use std::fs::File;
        use tiff::decoder::{Decoder, DecodingResult};

        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                eprintln!("[1kEE] SRTM open error {}: {e}", path.display());
                return None;
            }
        };
        let mut decoder = Decoder::new(BufReader::with_capacity(256 * 1024, file))
            .map_err(|e| eprintln!("[1kEE] SRTM TIFF init error {}: {e}", path.display()))
            .ok()?;

        let (width, height) = decoder
            .dimensions()
            .map_err(|e| eprintln!("[1kEE] SRTM dimensions error {}: {e}", path.display()))
            .ok()?;

        let result = decoder
            .read_image()
            .map_err(|e| eprintln!("[1kEE] SRTM read error {}: {e}", path.display()))
            .ok()?;

        let samples = match result {
            DecodingResult::I16(data) => SrtmSamples::I16(data),
            DecodingResult::U16(data) => {
                let signed: Vec<i16> = data.into_iter().map(|u| u as i16).collect();
                SrtmSamples::I16(signed)
            }
            DecodingResult::F32(mut data) => {
                for f in &mut data {
                    if f.is_nan() || *f <= -32767.0 {
                        *f = f32::NAN;
                    }
                }
                SrtmSamples::F32(data)
            }
            DecodingResult::F64(data) => SrtmSamples::F32(
                data.into_iter()
                    .map(|f| {
                        if f.is_nan() || f <= -32767.0 {
                            f32::NAN
                        } else {
                            f as f32
                        }
                    })
                    .collect(),
            ),
            _other => {
                eprintln!(
                    "[1kEE] SRTM unsupported pixel format in {} (not I16/U16/F32/F64)",
                    path.display()
                );
                return None;
            }
        };

        let len = match &samples {
            SrtmSamples::I16(v) => v.len(),
            SrtmSamples::F32(v) => v.len(),
        };
        if len != (width * height) as usize {
            eprintln!(
                "[1kEE] SRTM sample count mismatch {}: got {} expected {}",
                path.display(),
                len,
                width * height
            );
            return None;
        }

        Some(Self {
            width,
            height,
            samples,
        })
    }

    #[inline]
    fn get(&self, x: u32, y: u32) -> f32 {
        let idx = (y * self.width + x) as usize;
        match &self.samples {
            SrtmSamples::I16(data) => match data.get(idx).copied() {
                Some(s) if s != i16::MIN => s as f32,
                _ => f32::NAN,
            },
            SrtmSamples::F32(data) => data.get(idx).copied().unwrap_or(f32::NAN),
        }
    }

    /// Bilinear sample at (lat, lon).
    /// Assumes the tile covers [floor_lat, floor_lat+1) × [floor_lon, floor_lon+1).
    #[inline]
    fn sample(&self, lat: f32, lon: f32, lat_base: i32, lon_base: i32) -> f32 {
        let u = (lon - lon_base as f32).clamp(0.0, 0.999_999);
        let v = (1.0 - (lat - lat_base as f32)).clamp(0.0, 0.999_999);

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

        if !(tl + tr + bl + br).is_finite() {
            return f32::NAN;
        }

        let top = tl + (tr - tl) * tx;
        let bot = bl + (br - bl) * tx;
        top + (bot - top) * ty
    }
}

/// Thread-safe bounded LRU SRTM tile cache shared across Rayon workers.
pub struct SharedSrtmCache {
    root: PathBuf,
    lru: Mutex<LruCache<(i32, i32), Option<Arc<SrtmTile>>>>,
}

impl SharedSrtmCache {
    pub fn new(root: PathBuf, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            root,
            lru: Mutex::new(LruCache::new(capacity.max(8))),
        })
    }

    fn get_or_load(&self, key: (i32, i32)) -> Option<Arc<SrtmTile>> {
        if let Ok(mut guard) = self.lru.lock() {
            if let Some(hit) = guard.get(&key) {
                return hit.clone();
            }
        }
        let path = srtm_tile_path_for_key(&self.root, key.0, key.1);
        let loaded = SrtmTile::load(&path).map(Arc::new);
        if let Ok(mut guard) = self.lru.lock() {
            guard.insert(key, loaded.clone());
        }
        loaded
    }
}

/// Per-worker SRTM sampler backed by a shared LRU with a lock-free 1-slot
/// fast path (zero locks and zero heap allocations for 99.9%+ of pixels).
pub struct NativeSrtmSampler {
    shared: Arc<SharedSrtmCache>,
    last: Option<((i32, i32), Option<Arc<SrtmTile>>)>,
}

impl NativeSrtmSampler {
    pub fn new(root: PathBuf) -> Self {
        Self::with_shared(SharedSrtmCache::new(root, 16))
    }

    pub fn with_shared(shared: Arc<SharedSrtmCache>) -> Self {
        Self { shared, last: None }
    }

    #[inline]
    pub fn sample(&mut self, lat: f32, lon: f32) -> f32 {
        let key = (lat.floor() as i32, lon.floor() as i32);
        if let Some((last_key, ref tile)) = self.last
            && last_key == key
        {
            return match tile {
                Some(t) => t.sample(lat, lon, key.0, key.1),
                None => f32::NAN,
            };
        }
        let tile = self.shared.get_or_load(key);
        let v = match &tile {
            Some(t) => t.sample(lat, lon, key.0, key.1),
            None => f32::NAN,
        };
        self.last = Some((key, tile));
        v
    }
}

fn srtm_tile_path_for_key(root: &Path, lat_base: i32, lon_base: i32) -> PathBuf {
    let lat_prefix = if lat_base >= 0 { 'N' } else { 'S' };
    let lon_prefix = if lon_base >= 0 { 'E' } else { 'W' };
    root.join(format!(
        "{}{:02}{}{:03}.tif",
        lat_prefix,
        lat_base.unsigned_abs(),
        lon_prefix,
        lon_base.unsigned_abs(),
    ))
}

// ── Public types ──────────────────────────────────────────────────────────────

pub struct ContourLine {
    pub elevation_m: f32,
    pub points: Vec<(f32, f32)>, // (lon, lat)
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn build_tile_contours_with_sampler(
    spec: FocusContourSpec,
    bounds: GeoBounds,
    sample: impl FnMut(f32, f32) -> f32,
) -> (Vec<ContourLine>, Vec<Vec<(f32, f32)>>) {
    build_tile_contours_on_grid(
        spec,
        tile_archive::contour_grid::Bounds {
            min_lat: bounds.min_lat.into(),
            max_lat: bounds.max_lat.into(),
            min_lon: bounds.min_lon.into(),
            max_lon: bounds.max_lon.into(),
        },
        sample,
    )
}

pub fn build_tile_contours_on_grid(
    spec: FocusContourSpec,
    bounds: tile_archive::contour_grid::Bounds,
    mut sample: impl FnMut(f32, f32) -> f32,
) -> (Vec<ContourLine>, Vec<Vec<(f32, f32)>>) {
    let n = spec.raster_size as usize;
    let grid = build_grid(n, bounds, &mut sample);

    let (mut min_e, mut max_e) = (f32::INFINITY, f32::NEG_INFINITY);
    for &v in &grid {
        if !v.is_nan() {
            min_e = min_e.min(v);
            max_e = max_e.max(v);
        }
    }
    if min_e > max_e {
        return (Vec::new(), Vec::new()); // all nodata
    }

    let interval_m = spec.interval_m.max(1);
    let interval = interval_m as f32;
    let k_min = (min_e / interval).ceil() as i32;
    let k_max = (max_e / interval).floor() as i32;
    let num_levels = if k_min <= k_max {
        (k_max - k_min + 1) as usize
    } else {
        0
    };

    let mut coast_segs: Vec<[(f32, f32); 2]> = Vec::new();
    let mut level_segs: Vec<Vec<[(f32, f32); 2]>> = (0..num_levels).map(|_| Vec::new()).collect();

    // Single pass over all 2×2 cells: emit only the levels actually crossed by
    // each cell (instead of up to 800 full-grid passes) and compute only the
    // 2 active edge crossings per case.
    let dlat = (bounds.max_lat - bounds.min_lat) / n as f64;
    let dlon = (bounds.max_lon - bounds.min_lon) / n as f64;
    let lats: Vec<f32> = (0..n)
        .map(|row| (bounds.max_lat - (row as f64 + 0.5) * dlat) as f32)
        .collect();
    let lons: Vec<f32> = (0..n)
        .map(|col| (bounds.min_lon + (col as f64 + 0.5) * dlon) as f32)
        .collect();

    let need_coast = min_e <= 0.0 && max_e > 0.0;

    for row in 0..n.saturating_sub(1) {
        let lat0 = lats[row];
        let lat1 = lats[row + 1];
        let row0 = &grid[row * n..(row + 1) * n];
        let row1 = &grid[(row + 1) * n..(row + 2) * n];
        for col in 0..n.saturating_sub(1) {
            let tl = row0[col];
            let tr = row0[col + 1];
            let br = row1[col + 1];
            let bl = row1[col];
            if !(tl + tr + br + bl).is_finite() {
                continue;
            }
            let cell_min = tl.min(tr).min(br).min(bl);
            let cell_max = tl.max(tr).max(br).max(bl);
            if cell_min == cell_max {
                continue;
            }
            let lon0 = lons[col];
            let lon1 = lons[col + 1];

            if need_coast && cell_min <= 0.0 && cell_max > 0.0 {
                emit_cell_segments(
                    tl,
                    tr,
                    br,
                    bl,
                    0.0,
                    lon0,
                    lon1,
                    lat0,
                    lat1,
                    &mut coast_segs,
                );
            }

            if num_levels > 0 {
                let c_lo = ((cell_min / interval).ceil() as i32).max(k_min);
                let c_hi = ((cell_max / interval).floor() as i32).min(k_max);
                for k in c_lo..=c_hi {
                    if k == 0 {
                        continue;
                    }
                    let threshold = (k * interval_m) as f32;
                    if cell_min <= threshold && cell_max > threshold {
                        emit_cell_segments(
                            tl,
                            tr,
                            br,
                            bl,
                            threshold,
                            lon0,
                            lon1,
                            lat0,
                            lat1,
                            &mut level_segs[(k - k_min) as usize],
                        );
                    }
                }
            }
        }
    }

    let coastlines = if coast_segs.is_empty() {
        Vec::new()
    } else {
        chain_segments(coast_segs)
            .into_iter()
            .filter(|p| p.len() >= 2)
            .collect()
    };

    let mut contours = Vec::new();
    for (offset, segs) in level_segs.into_iter().enumerate() {
        if segs.is_empty() {
            continue;
        }
        let level = (k_min + offset as i32) * interval_m;
        if level == 0 {
            continue;
        }
        let threshold = level as f32;
        for poly in chain_segments(segs) {
            if poly.len() >= 2 {
                contours.push(ContourLine {
                    elevation_m: threshold,
                    points: poly,
                });
            }
        }
    }

    (contours, coastlines)
}

/// Build all contour lines for one tile.
///
/// Returns `(contours, coastlines)` where `coastlines` are the 0 m iso-lines.
/// Both vecs may be empty for ocean / nodata tiles.
pub fn build_tile_contours(
    sampler: &mut NativeSrtmSampler,
    spec: FocusContourSpec,
    bounds: GeoBounds,
) -> (Vec<ContourLine>, Vec<Vec<(f32, f32)>>) {
    build_tile_contours_with_sampler(spec, bounds, |lat, lon| sampler.sample(lat, lon))
}

// ── Grid building ─────────────────────────────────────────────────────────────

fn build_grid(
    n: usize,
    bounds: tile_archive::contour_grid::Bounds,
    sample: &mut dyn FnMut(f32, f32) -> f32,
) -> Vec<f32> {
    let dlat = (bounds.max_lat - bounds.min_lat) / n as f64;
    let dlon = (bounds.max_lon - bounds.min_lon) / n as f64;
    let mut grid = Vec::with_capacity(n * n);
    for row in 0..n {
        let lat = bounds.max_lat - (row as f64 + 0.5) * dlat;
        for col in 0..n {
            let lon = bounds.min_lon + (col as f64 + 0.5) * dlon;
            grid.push(sample(lat as f32, lon as f32));
        }
    }
    grid
}

// ── Marching squares ──────────────────────────────────────────────────────────
//
// Bit convention:  8=TL  4=TR  2=BR  1=BL  (set = above threshold)
// Edge names:      N=top  E=right  S=bottom  W=left

#[inline]
#[allow(clippy::too_many_arguments)]
fn emit_cell_segments(
    tl: f32,
    tr: f32,
    br: f32,
    bl: f32,
    threshold: f32,
    lon0: f32,
    lon1: f32,
    lat0: f32,
    lat1: f32,
    segs: &mut Vec<[(f32, f32); 2]>,
) {
    let idx = ((tl > threshold) as u8) << 3
        | ((tr > threshold) as u8) << 2
        | ((br > threshold) as u8) << 1
        | ((bl > threshold) as u8);

    if idx == 0 || idx == 15 {
        return;
    }

    let n_pt = || (lon0 + interp(tl, tr, threshold) * (lon1 - lon0), lat0);
    let e_pt = || (lon1, lat0 + interp(tr, br, threshold) * (lat1 - lat0));
    let s_pt = || (lon0 + interp(bl, br, threshold) * (lon1 - lon0), lat1);
    let w_pt = || (lon0, lat0 + interp(tl, bl, threshold) * (lat1 - lat0));

    match idx {
        1 | 14 => segs.push([w_pt(), s_pt()]),
        2 | 13 => segs.push([s_pt(), e_pt()]),
        3 | 12 => segs.push([w_pt(), e_pt()]),
        4 | 11 => segs.push([n_pt(), e_pt()]),
        5 => {
            let n = n_pt();
            let e = e_pt();
            let s = s_pt();
            let w = w_pt();
            let avg = (tl + tr + br + bl) * 0.25;
            if avg > threshold {
                segs.push([n, w]);
                segs.push([e, s]);
            } else {
                segs.push([n, e]);
                segs.push([w, s]);
            }
        }
        6 | 9 => segs.push([n_pt(), s_pt()]),
        7 | 8 => segs.push([n_pt(), w_pt()]),
        10 => {
            let n = n_pt();
            let e = e_pt();
            let s = s_pt();
            let w = w_pt();
            let avg = (tl + tr + br + bl) * 0.25;
            if avg > threshold {
                segs.push([n, e]);
                segs.push([w, s]);
            } else {
                segs.push([n, w]);
                segs.push([e, s]);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(crate) fn extract_segments(
    grid: &[f32],
    n: usize,
    threshold: f32,
    bounds: tile_archive::contour_grid::Bounds,
) -> Vec<[(f32, f32); 2]> {
    let dlat = (bounds.max_lat - bounds.min_lat) / n as f64;
    let dlon = (bounds.max_lon - bounds.min_lon) / n as f64;
    let lats: Vec<f32> = (0..n)
        .map(|row| (bounds.max_lat - (row as f64 + 0.5) * dlat) as f32)
        .collect();
    let lons: Vec<f32> = (0..n)
        .map(|col| (bounds.min_lon + (col as f64 + 0.5) * dlon) as f32)
        .collect();
    let mut segs: Vec<[(f32, f32); 2]> = Vec::new();
    for row in 0..n.saturating_sub(1) {
        let lat0 = lats[row];
        let lat1 = lats[row + 1];
        for col in 0..n.saturating_sub(1) {
            let tl = grid[row * n + col];
            let tr = grid[row * n + col + 1];
            let br = grid[(row + 1) * n + col + 1];
            let bl = grid[(row + 1) * n + col];
            if !(tl + tr + br + bl).is_finite() {
                continue;
            }
            emit_cell_segments(
                tl,
                tr,
                br,
                bl,
                threshold,
                lons[col],
                lons[col + 1],
                lat0,
                lat1,
                &mut segs,
            );
        }
    }
    segs
}

fn interp(v0: f32, v1: f32, threshold: f32) -> f32 {
    let dv = v1 - v0;
    if dv.abs() < 1e-7 {
        0.5
    } else {
        ((threshold - v0) / dv).clamp(0.0, 1.0)
    }
}

// ── Segment chaining ──────────────────────────────────────────────────────────

pub(crate) fn chain_segments(segments: Vec<[(f32, f32); 2]>) -> Vec<Vec<(f32, f32)>> {
    if segments.is_empty() {
        return Vec::new();
    }
    const EMPTY: u32 = u32::MAX;
    // Compact [u32; 2] inline slots handle 99.9%+ of contour vertices (degree <= 2)
    // without heap-allocating SmallVecs; degree > 2 saddles spill into `overflow`.
    let mut ep_map: FxHashMap<u64, [u32; 2]> =
        FxHashMap::with_capacity_and_hasher(segments.len() + 16, Default::default());
    let mut overflow: FxHashMap<u64, Vec<u32>> = FxHashMap::default();
    for (i, seg) in segments.iter().enumerate() {
        let idx = i as u32;
        for &pt in seg {
            let key = ebits(pt);
            let slot = ep_map.entry(key).or_insert([EMPTY, EMPTY]);
            if slot[0] == EMPTY {
                slot[0] = idx;
            } else if slot[1] == EMPTY {
                slot[1] = idx;
            } else {
                overflow.entry(key).or_default().push(idx);
            }
        }
    }

    let mut used = vec![false; segments.len()];
    let mut polylines = Vec::new();
    let mut poly: VecDeque<(f32, f32)> = VecDeque::new();

    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;

        poly.clear();
        poly.push_back(segments[start][0]);
        poly.push_back(segments[start][1]);

        // Extend from tail
        loop {
            let tail = *poly.back().unwrap();
            let tail_bits = ebits(tail);
            match find_next(&ep_map, &overflow, &used, tail_bits) {
                None => break,
                Some(next_idx) => {
                    used[next_idx] = true;
                    let seg = &segments[next_idx];
                    let new_pt = if ebits(seg[0]) == tail_bits {
                        seg[1]
                    } else {
                        seg[0]
                    };
                    poly.push_back(new_pt);
                }
            }
        }

        // Extend from head
        loop {
            let head = *poly.front().unwrap();
            let head_bits = ebits(head);
            match find_next(&ep_map, &overflow, &used, head_bits) {
                None => break,
                Some(next_idx) => {
                    used[next_idx] = true;
                    let seg = &segments[next_idx];
                    let new_pt = if ebits(seg[0]) == head_bits {
                        seg[1]
                    } else {
                        seg[0]
                    };
                    poly.push_front(new_pt);
                }
            }
        }

        if poly.len() >= 2 {
            let mut v = Vec::with_capacity(poly.len());
            let (front, back) = poly.as_slices();
            v.extend_from_slice(front);
            v.extend_from_slice(back);
            polylines.push(v);
        }
    }

    polylines
}

#[inline]
fn ebits(p: (f32, f32)) -> u64 {
    ((p.0.to_bits() as u64) << 32) | (p.1.to_bits() as u64)
}

#[inline]
fn find_next(
    ep_map: &FxHashMap<u64, [u32; 2]>,
    overflow: &FxHashMap<u64, Vec<u32>>,
    used: &[bool],
    key: u64,
) -> Option<usize> {
    let slot = ep_map.get(&key)?;
    if slot[0] != u32::MAX && !used[slot[0] as usize] {
        return Some(slot[0] as usize);
    }
    if slot[1] != u32::MAX && !used[slot[1] as usize] {
        return Some(slot[1] as usize);
    }
    overflow
        .get(&key)?
        .iter()
        .find(|&&i| !used[i as usize])
        .map(|&i| i as usize)
}
