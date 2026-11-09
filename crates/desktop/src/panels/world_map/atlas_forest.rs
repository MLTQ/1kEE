//! Geographic forest coverage, applied to the actual Atlas elevation surface.
use super::{
    atlas_surface::Raster, cell_loader::load_features_from_cells, layer_snapshot::Snapshot,
};
use crate::{model::GeoPoint, osm_ingest};
use std::{
    collections::{HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

pub(super) struct Mask {
    pub id: u64,
    pub raster_id: u64,
    pub size: [usize; 2],
    pub pixels: Vec<u8>,
}
impl Mask {
    pub fn new(raster_id: u64, size: [usize; 2], pixels: Vec<u8>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            raster_id,
            size,
            pixels,
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Request {
    root: PathBuf,
    raster_id: u64,
    bounds: [u32; 4],
    size: [usize; 2],
    generation: u64,
}
static CACHE: OnceLock<Mutex<Snapshot<Request, Mask>>> = OnceLock::new();
static WANTED: AtomicU64 = AtomicU64::new(0);
pub(super) fn suspend() {
    WANTED.store(0, Ordering::Relaxed);
}

pub(super) fn get(ctx: &egui::Context, root: Option<&Path>, raster: &Raster) -> Option<Arc<Mask>> {
    WANTED.store(raster.id, Ordering::Relaxed);
    let request = Request {
        root: root?.into(),
        raster_id: raster.id,
        bounds: raster.bounds.map(f32::to_bits),
        size: raster.size,
        generation: osm_ingest::road_data_generation(),
    };
    let result = CACHE.get_or_init(Default::default).lock().ok()?.get(
        request,
        "atlas-forest",
        |a, b| a.root == b.root && a.raster_id == b.raster_id,
        |request| {
            let bounds = request.bounds.map(f32::from_bits);
            let mut pixels = vec![0; request.size[0] * request.size[1]];
            let span = (bounds[2] - bounds[0]).min(360.);
            let west = (bounds[0] + 180.).rem_euclid(360.) - 180.;
            let east = west + span;
            let mut windows = vec![(west, east.min(180.))];
            if east > 180. {
                windows.push((-180., east - 360.));
            }
            let mut seen = HashSet::new();
            // Keep only one source cell in memory, and abandon an obsolete view
            // between cells instead of reading its entire vector window.
            for (west, east) in windows {
                for lat in bounds[1].floor() as i32..=bounds[3].floor() as i32 {
                    for lon in west.floor() as i32..=east.floor() as i32 {
                        if WANTED.load(Ordering::Relaxed) != request.raster_id {
                            return None;
                        }
                        let cell = osm_ingest::GeoBounds {
                            min_lat: bounds[1].max(lat as f32),
                            max_lat: bounds[3].min(cell_end(lat)),
                            min_lon: west.max(lon as f32),
                            max_lon: east.min(cell_end(lon)),
                        };
                        if cell.min_lat >= cell.max_lat || cell.min_lon >= cell.max_lon {
                            continue;
                        }
                        for feature in load_features_from_cells(&request.root, "tree", cell) {
                            if !feature.is_polygon {
                                continue;
                            }
                            let mut hash = DefaultHasher::new();
                            for point in &feature.points {
                                point.lon.to_bits().hash(&mut hash);
                                point.lat.to_bits().hash(&mut hash);
                            }
                            if seen.insert((feature.way_id, feature.points.len(), hash.finish())) {
                                paint_ring(&mut pixels, request.size, bounds, &feature.points);
                            }
                        }
                    }
                }
            }
            if WANTED.load(Ordering::Relaxed) != request.raster_id {
                return None;
            }
            Some(Mask::new(request.raster_id, request.size, pixels))
        },
    );
    if result.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    result
}
pub(super) fn clear() {
    suspend();
    if let Some(cache) = CACHE.get() {
        cache.lock().unwrap().clear();
    }
}
#[cfg(test)]
pub(super) fn uploaded(ctx: &egui::Context, id: u64) -> bool {
    ctx.data(|d| d.get_temp::<u64>(egui::Id::new("atlas-forest-active"))) == Some(id)
}

// The shared loader enumerates inclusive integer cells. Use the immediately
// preceding float so one cell read does not silently include its neighbors.
fn cell_end(cell: i32) -> f32 {
    let boundary = (cell + 1) as f32;
    if boundary == 0.0 {
        -f32::from_bits(1)
    } else if boundary > 0.0 {
        f32::from_bits(boundary.to_bits() - 1)
    } else {
        f32::from_bits(boundary.to_bits() + 1)
    }
}

/// Pixel-center, even-odd scan conversion: arbitrary concavity is preserved.
/// Repeated source cells are idempotent coverage rather than stacked opacity.
fn paint_ring(pixels: &mut [u8], size: [usize; 2], bounds: [f32; 4], ring: &[GeoPoint]) {
    if ring.len() < 4
        || ring.first() != ring.last()
        || ring
            .iter()
            .any(|p| !p.lon.is_finite() || !p.lat.is_finite())
    {
        return;
    }
    let center = (bounds[0] + bounds[2]) * 0.5;
    let mut previous = ring[0].lon;
    let shift = ((center - previous) / 360.).round() * 360.;
    let mut coordinates = Vec::with_capacity(ring.len());
    for p in ring {
        let lon = p.lon + ((previous - p.lon) / 360.).round() * 360.;
        previous = lon;
        coordinates.push((
            ((lon + shift) as f64 - bounds[0] as f64) / (bounds[2] - bounds[0]) as f64
                * size[0] as f64,
            (bounds[3] as f64 - p.lat as f64) / (bounds[3] - bounds[1]) as f64 * size[1] as f64,
        ));
    }
    let min_y = coordinates
        .iter()
        .map(|p| p.1)
        .fold(f64::INFINITY, f64::min);
    let max_y = coordinates
        .iter()
        .map(|p| p.1)
        .fold(f64::NEG_INFINITY, f64::max);
    let first = ((min_y - 0.5).ceil() as i64).clamp(0, size[1] as i64) as usize;
    let last = ((max_y - 0.5).ceil() as i64).clamp(0, size[1] as i64) as usize;
    let mut crossings = Vec::new();
    for y in first..last {
        crossings.clear();
        let scan = y as f64 + 0.5;
        for edge in coordinates.windows(2) {
            let [(ax, ay), (bx, by)] = [edge[0], edge[1]];
            if (ay > scan) != (by > scan) {
                crossings.push(ax + (scan - ay) / (by - ay) * (bx - ax));
            }
        }
        crossings.sort_by(f64::total_cmp);
        for pair in crossings.chunks_exact(2) {
            let left = ((pair[0] - 0.5).ceil() as i64).clamp(0, size[0] as i64) as usize;
            let right = ((pair[1] - 0.5).ceil() as i64).clamp(0, size[0] as i64) as usize;
            pixels[y * size[0] + left..y * size[0] + right].fill(255);
        }
    }
}

#[cfg(test)]
#[path = "atlas_forest_tests.rs"]
mod tests;
