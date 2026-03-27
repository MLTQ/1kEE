/// Pure-Rust contour extraction using marching squares.
///
/// Reads SRTM elevation tiles, builds a raster grid at the tile spec's
/// resolution, and extracts iso-lines at every `interval_m` multiple.
/// Produces the same output schema as the GDAL pipeline so the desktop
/// reads both transparently.
use crate::contours::{FocusContourSpec, GeoBounds};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

// ── SRTM tile reader (NaN for nodata / missing tiles) ─────────────────────────

struct SrtmTile {
    width: u32,
    height: u32,
    /// NaN for SRTM nodata (-32768); otherwise elevation in metres.
    samples: Vec<f32>,
}

impl SrtmTile {
    /// Load an SRTM GeoTIFF tile.  Handles both signed Int16 (the most common
    /// SRTM distribution format) and Float32 (some re-processed products).
    ///
    /// Using the `tiff` crate directly rather than going through `image` avoids
    /// a silent failure mode where `image` rejects Int16 TIFFs tagged with
    /// SampleFormat=2 (SAMPLEFORMAT_INT) and `decode()` returns an error.
    fn load(path: &Path) -> Option<Self> {
        use tiff::decoder::{Decoder, DecodingResult};
        use std::fs::File;

        let file = File::open(path)
            .map_err(|e| eprintln!("[1kEE] SRTM open error {}: {e}", path.display()))
            .ok()?;
        let mut decoder = Decoder::new(file)
            .map_err(|e| eprintln!("[1kEE] SRTM TIFF init error {}: {e}", path.display()))
            .ok()?;

        let (width, height) = decoder.dimensions()
            .map_err(|e| eprintln!("[1kEE] SRTM dimensions error {}: {e}", path.display()))
            .ok()?;

        let result = decoder.read_image()
            .map_err(|e| eprintln!("[1kEE] SRTM read error {}: {e}", path.display()))
            .ok()?;

        let samples: Vec<f32> = match result {
            // Signed Int16 — the canonical SRTM format (SampleFormat=2).
            // -32768 is the SRTM nodata sentinel; map to NaN.
            DecodingResult::I16(data) => data
                .into_iter()
                .map(|s| if s == i16::MIN { f32::NAN } else { s as f32 })
                .collect(),
            // Unsigned 16-bit — sometimes written by tools that ignore SampleFormat.
            // Reinterpret bits as i16 so negative elevations decode correctly.
            DecodingResult::U16(data) => data
                .into_iter()
                .map(|u| {
                    let s = u as i16;
                    if s == i16::MIN { f32::NAN } else { s as f32 }
                })
                .collect(),
            // Float32 — used by some re-projected SRTM products.
            // Common nodata values: NaN, -32768.0, -9999.0.
            DecodingResult::F32(data) => data
                .into_iter()
                .map(|f| {
                    if f.is_nan() || f <= -32767.0 { f32::NAN } else { f }
                })
                .collect(),
            // Float64 — rare but possible after reprojection.
            DecodingResult::F64(data) => data
                .into_iter()
                .map(|f| {
                    if f.is_nan() || f <= -32767.0 { f32::NAN } else { f as f32 }
                })
                .collect(),
            _other => {
                eprintln!(
                    "[1kEE] SRTM unsupported pixel format in {} (not I16/U16/F32/F64)",
                    path.display()
                );
                return None;
            }
        };

        if samples.len() != (width * height) as usize {
            eprintln!(
                "[1kEE] SRTM sample count mismatch {}: got {} expected {}",
                path.display(), samples.len(), width * height
            );
            return None;
        }

        Some(Self { width, height, samples })
    }

    fn get(&self, x: u32, y: u32) -> f32 {
        self.samples
            .get((y * self.width + x) as usize)
            .copied()
            .unwrap_or(f32::NAN)
    }

    /// Bilinear sample at (lat, lon).
    /// Assumes the tile covers [floor_lat, floor_lat+1) × [floor_lon, floor_lon+1).
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

        if tl.is_nan() || tr.is_nan() || bl.is_nan() || br.is_nan() {
            return f32::NAN;
        }

        let top = tl + (tr - tl) * tx;
        let bot = bl + (br - bl) * tx;
        top + (bot - top) * ty
    }
}

/// LRU SRTM tile cache.  Returns NaN for missing tiles and nodata cells,
/// unlike `SrtmSampler` in `srtm.rs` (which converts -32768 → 0.0).
pub struct NativeSrtmSampler {
    root: PathBuf,
    tiles: Vec<(PathBuf, SrtmTile)>,
    missing: HashSet<PathBuf>,
}

impl NativeSrtmSampler {
    pub fn new(root: PathBuf) -> Self {
        Self { root, tiles: Vec::new(), missing: HashSet::new() }
    }

    pub fn sample(&mut self, lat: f32, lon: f32) -> f32 {
        let path = srtm_tile_path(&self.root, lat, lon);
        if self.missing.contains(&path) {
            return f32::NAN;
        }
        if let Some(idx) = self.tiles.iter().position(|(p, _)| *p == path) {
            let entry = self.tiles.remove(idx);
            let v = entry.1.sample(lat, lon);
            self.tiles.insert(0, entry);
            return v;
        }
        match SrtmTile::load(&path) {
            Some(tile) => {
                let v = tile.sample(lat, lon);
                self.tiles.insert(0, (path, tile));
                if self.tiles.len() > 8 {
                    self.tiles.pop();
                }
                v
            }
            None => {
                self.missing.insert(path);
                f32::NAN
            }
        }
    }
}

fn srtm_tile_path(root: &Path, lat: f32, lon: f32) -> PathBuf {
    let lat_base = lat.floor() as i32;
    let lon_base = lon.floor() as i32;
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

/// Build all contour lines for one tile.
///
/// Returns `(contours, coastlines)` where `coastlines` are the 0 m iso-lines.
/// Both vecs may be empty for ocean / nodata tiles.
pub fn build_tile_contours(
    sampler: &mut NativeSrtmSampler,
    spec: FocusContourSpec,
    bounds: GeoBounds,
) -> (Vec<ContourLine>, Vec<Vec<(f32, f32)>>) {
    let n = spec.raster_size as usize;
    let grid = build_grid(sampler, n, bounds);

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

    // Coastline (0 m iso-line) handled separately
    let coast_segs = extract_segments(&grid, n, 0.0, bounds);
    let coastlines = if coast_segs.is_empty() {
        Vec::new()
    } else {
        chain_segments(coast_segs)
            .into_iter()
            .filter(|p| p.len() >= 2)
            .collect()
    };

    // Contour intervals
    let interval = spec.interval_m as f32;
    let lo = ((min_e / interval).ceil() as i32) * spec.interval_m;
    let hi = ((max_e / interval).floor() as i32) * spec.interval_m;

    let mut contours = Vec::new();
    let mut level = lo;
    while level <= hi {
        if level != 0 {
            let threshold = level as f32;
            let segs = extract_segments(&grid, n, threshold, bounds);
            if !segs.is_empty() {
                for poly in chain_segments(segs) {
                    if poly.len() >= 2 {
                        contours.push(ContourLine { elevation_m: threshold, points: poly });
                    }
                }
            }
        }
        level += spec.interval_m;
    }

    (contours, coastlines)
}

// ── Grid building ─────────────────────────────────────────────────────────────

fn build_grid(sampler: &mut NativeSrtmSampler, n: usize, bounds: GeoBounds) -> Vec<f32> {
    let dlat = (bounds.max_lat - bounds.min_lat) / n as f32;
    let dlon = (bounds.max_lon - bounds.min_lon) / n as f32;
    let mut grid = Vec::with_capacity(n * n);
    for row in 0..n {
        let lat = bounds.max_lat - (row as f32 + 0.5) * dlat;
        for col in 0..n {
            let lon = bounds.min_lon + (col as f32 + 0.5) * dlon;
            grid.push(sampler.sample(lat, lon));
        }
    }
    grid
}

// ── Marching squares ──────────────────────────────────────────────────────────
//
// Bit convention:  8=TL  4=TR  2=BR  1=BL  (set = above threshold)
// Edge names:      N=top  E=right  S=bottom  W=left

fn extract_segments(grid: &[f32], n: usize, threshold: f32, bounds: GeoBounds) -> Vec<[(f32, f32); 2]> {
    let dlat = (bounds.max_lat - bounds.min_lat) / n as f32;
    let dlon = (bounds.max_lon - bounds.min_lon) / n as f32;

    let lat_at = |row: usize| bounds.max_lat - (row as f32 + 0.5) * dlat;
    let lon_at = |col: usize| bounds.min_lon + (col as f32 + 0.5) * dlon;
    let g = |row: usize, col: usize| grid[row * n + col];

    let mut segs: Vec<[(f32, f32); 2]> = Vec::new();

    for row in 0..n - 1 {
        for col in 0..n - 1 {
            let tl = g(row, col);
            let tr = g(row, col + 1);
            let br = g(row + 1, col + 1);
            let bl = g(row + 1, col);

            if tl.is_nan() || tr.is_nan() || br.is_nan() || bl.is_nan() {
                continue;
            }

            let idx = ((tl > threshold) as u8) << 3
                    | ((tr > threshold) as u8) << 2
                    | ((br > threshold) as u8) << 1
                    | ((bl > threshold) as u8);

            if idx == 0 || idx == 15 {
                continue;
            }

            let lat0 = lat_at(row);
            let lat1 = lat_at(row + 1);
            let lon0 = lon_at(col);
            let lon1 = lon_at(col + 1);

            // Interpolated edge crossing points
            let n_pt = {
                let t = interp(tl, tr, threshold);
                (lon0 + t * (lon1 - lon0), lat0)
            };
            let e_pt = {
                let t = interp(tr, br, threshold);
                (lon1, lat0 + t * (lat1 - lat0))
            };
            let s_pt = {
                let t = interp(bl, br, threshold);
                (lon0 + t * (lon1 - lon0), lat1)
            };
            let w_pt = {
                let t = interp(tl, bl, threshold);
                (lon0, lat0 + t * (lat1 - lat0))
            };

            match idx {
                1  => segs.push([w_pt, s_pt]),
                2  => segs.push([s_pt, e_pt]),
                3  => segs.push([w_pt, e_pt]),
                4  => segs.push([n_pt, e_pt]),
                5  => {
                    // Saddle: TR+BL above, TL+BR below
                    let avg = (tl + tr + br + bl) * 0.25;
                    if avg > threshold {
                        segs.push([n_pt, w_pt]);
                        segs.push([e_pt, s_pt]);
                    } else {
                        segs.push([n_pt, e_pt]);
                        segs.push([w_pt, s_pt]);
                    }
                }
                6  => segs.push([n_pt, s_pt]),
                7  => segs.push([n_pt, w_pt]),
                8  => segs.push([n_pt, w_pt]),
                9  => segs.push([n_pt, s_pt]),
                10 => {
                    // Saddle: TL+BR above, TR+BL below
                    let avg = (tl + tr + br + bl) * 0.25;
                    if avg > threshold {
                        segs.push([n_pt, e_pt]);
                        segs.push([w_pt, s_pt]);
                    } else {
                        segs.push([n_pt, w_pt]);
                        segs.push([e_pt, s_pt]);
                    }
                }
                11 => segs.push([n_pt, e_pt]),
                12 => segs.push([w_pt, e_pt]),
                13 => segs.push([s_pt, e_pt]),
                14 => segs.push([w_pt, s_pt]),
                _  => {}
            }
        }
    }
    segs
}

fn interp(v0: f32, v1: f32, threshold: f32) -> f32 {
    let dv = v1 - v0;
    if dv.abs() < 1e-7 { 0.5 } else { ((threshold - v0) / dv).clamp(0.0, 1.0) }
}

// ── Segment chaining ──────────────────────────────────────────────────────────

fn chain_segments(segments: Vec<[(f32, f32); 2]>) -> Vec<Vec<(f32, f32)>> {
    // Map: bit-exact endpoint key → segment indices sharing that endpoint.
    let mut ep_map: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, seg) in segments.iter().enumerate() {
        for &pt in seg {
            ep_map.entry(ebits(pt)).or_default().push(i);
        }
    }

    let mut used = vec![false; segments.len()];
    let mut polylines = Vec::new();

    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;

        let mut poly: VecDeque<(f32, f32)> = VecDeque::new();
        poly.push_back(segments[start][0]);
        poly.push_back(segments[start][1]);

        // Extend from tail
        loop {
            let tail = *poly.back().unwrap();
            match find_next(&ep_map, &used, tail) {
                None => break,
                Some(next_idx) => {
                    used[next_idx] = true;
                    let seg = &segments[next_idx];
                    let new_pt =
                        if ebits(seg[0]) == ebits(tail) { seg[1] } else { seg[0] };
                    poly.push_back(new_pt);
                }
            }
        }

        // Extend from head
        loop {
            let head = *poly.front().unwrap();
            match find_next(&ep_map, &used, head) {
                None => break,
                Some(next_idx) => {
                    used[next_idx] = true;
                    let seg = &segments[next_idx];
                    let new_pt =
                        if ebits(seg[0]) == ebits(head) { seg[1] } else { seg[0] };
                    poly.push_front(new_pt);
                }
            }
        }

        let v: Vec<(f32, f32)> = poly.into_iter().collect();
        if v.len() >= 2 {
            polylines.push(v);
        }
    }

    polylines
}

fn ebits(p: (f32, f32)) -> (u32, u32) {
    (p.0.to_bits(), p.1.to_bits())
}

fn find_next(
    ep_map: &HashMap<(u32, u32), Vec<usize>>,
    used: &[bool],
    pt: (f32, f32),
) -> Option<usize> {
    ep_map
        .get(&ebits(pt))?
        .iter()
        .find(|&&i| !used[i])
        .copied()
}
