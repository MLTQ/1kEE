//! Projected visibility and byte-budgeted globe terrain LRU retention.
use super::*;
use crate::model::{ActiveBody, GlobeViewState};
use crate::panels::world_map::{
    globe_scene::GlobeLayout, local_contour_pass::LocalTileId, local_terrain_scene::composition,
};
use tile_archive::contour_grid::Bounds as Rect;

pub(super) const GPU_BUDGET: usize = 256 * 1024 * 1024;
const CPU_BUDGET: usize = 384 * 1024 * 1024;

#[derive(Clone, Copy, Default)]
pub(super) struct Cost {
    gpu: usize,
    cpu: usize,
    used: u64,
}

impl Cost {
    pub fn measure(contours: &[ContourPath]) -> Self {
        Self {
            gpu: gpu_bytes(contours),
            cpu: std::mem::size_of_val(contours)
                + contours
                    .iter()
                    .map(|p| p.points.capacity() * std::mem::size_of::<GeoPoint>())
                    .sum::<usize>(),
            used: 0,
        }
    }
}

pub(super) fn gpu_bytes(contours: &[ContourPath]) -> usize {
    contours
        .iter()
        .map(|p| p.points.len().saturating_sub(1))
        .sum::<usize>()
        .saturating_mul(std::mem::size_of::<
            crate::panels::world_map::contour_pass::SegmentInstance,
        >())
}

#[derive(Clone, Copy)]
pub(crate) struct Viewport {
    yaw: [f64; 2],
    pitch: [f64; 2],
    planes: [[f64; 4]; 6],
}

impl Viewport {
    pub(crate) fn new(layout: &GlobeLayout, view: &GlobeViewState, rect: egui::Rect) -> Self {
        let rect = rect.expand(8.0);
        let f = f64::from(layout.radius * layout.focal_length);
        let d = f64::from(layout.camera_distance);
        let l = f64::from(layout.center.x - rect.left());
        let r = f64::from(rect.right() - layout.center.x);
        let t = f64::from(layout.center.y - rect.top());
        let b = f64::from(rect.bottom() - layout.center.y);
        Self {
            yaw: [f64::from(view.yaw.sin()), f64::from(view.yaw.cos())],
            pitch: [f64::from(view.pitch.sin()), f64::from(view.pitch.cos())],
            planes: [
                [-f, 0.0, -l, l * d],
                [f, 0.0, -r, r * d],
                [0.0, -f, -t, t * d],
                [0.0, f, -b, b * d],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, -1.0, d - 0.05],
            ],
        }
    }

    pub(crate) fn intersects(self, rect: Rect) -> bool {
        let lat = ((rect.min_lat + rect.max_lat) * 0.5).to_radians();
        let lon = ((rect.min_lon + rect.max_lon) * 0.5).to_radians();
        let x = lat.cos() * lon.cos();
        let y = lat.sin();
        let z = lat.cos() * lon.sin();
        let [ys, yc] = self.yaw;
        let [ps, pc] = self.pitch;
        let x1 = x * yc + z * ys;
        let z1 = -x * ys + z * yc;
        let center = [
            x1 * 1.02,
            (y * pc - z1 * ps) * 1.02,
            (y * ps + z1 * pc) * 1.02,
        ];
        // Angular distance via latitude then longitude upper-bounds every point
        // of the patch, including curved edges, the date line and the poles.
        let angle = ((rect.max_lat - rect.min_lat).abs() + (rect.max_lon - rect.min_lon).abs())
            .to_radians()
            * 0.5;
        let radius = 2.04 * (angle.min(std::f64::consts::PI) * 0.5).sin() + 0.006;
        self.planes.iter().all(|p| {
            let signed = p[0] * center[0] + p[1] * center[1] + p[2] * center[2] + p[3];
            let norm = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            !signed.is_finite() || signed + radius * norm >= 0.0
        })
    }
}

#[derive(Default)]
pub(super) struct Residency {
    pub view: Option<Viewport>,
    window: Option<(i32, i32, i32)>, // tier, center latitude/longitude bucket
    bounds: HashMap<(i32, i32), Option<residency::Bounds>>,
    pub cells: HashSet<(i32, i32)>,
    pub regions: Vec<Rect>,
    pub revision: u64,
    costs: HashMap<(i32, i32), Cost>,
    clock: u64,
    pub gpu_budget: usize,
    pub cpu_budget: usize,
    gpu_factor: f64,
    fallback_gpu: usize,
}

impl Residency {
    pub fn reset_sources(&mut self) {
        *self = Self {
            view: self.view,
            ..Default::default()
        };
    }
    pub fn configure(&mut self, bucket: i32, center: (i32, i32)) -> bool {
        let window = (bucket, center.0, center.1);
        if self.window.is_some_and(|w| w.0 != bucket) {
            self.bounds.clear();
            self.costs.clear();
        }
        self.window = Some(window);
        self.clock = self.clock.wrapping_add(1);
        if self.gpu_budget == 0 {
            self.gpu_budget = GPU_BUDGET;
        }
        if self.cpu_budget == 0 {
            self.cpu_budget = CPU_BUDGET;
        }
        self.bounds.retain(|&(lat, lon), _| {
            self.costs.contains_key(&(lat, lon))
                || ((lat - center.0).abs() <= 11 && (lon - center.1).abs() <= 11)
        });
        let mut cells = HashSet::new();
        let mut regions = Vec::new();
        for lat in center.0 - 8..=center.0 + 8 {
            for lon in center.1 - 8..=center.1 + 8 {
                let r = core(bucket, (lat, lon));
                if self.view.is_none_or(|v| v.intersects(r)) {
                    cells.insert((lat, lon));
                    regions.push(r);
                }
            }
        }
        if cells == self.cells {
            return false;
        }
        self.cells = cells;
        self.regions = regions;
        self.revision = self.revision.wrapping_add(1);
        true
    }

    pub fn record(&mut self, cell: (i32, i32), bounds: Option<residency::Bounds>) {
        if self.window.is_some() {
            self.bounds.insert(cell, bounds);
        }
    }

    pub fn admit(&mut self, cell: (i32, i32), mut cost: Cost) {
        if self.window.is_none() {
            return;
        }
        cost.used = self.clock;
        self.costs.insert(cell, cost);
    }

    pub fn observe_gpu(&mut self, bytes: usize, fallback: usize) {
        self.fallback_gpu = fallback;
        let raw: usize = self.costs.values().map(|c| c.gpu).sum();
        if raw > 0 {
            self.gpu_factor = (bytes.saturating_sub(fallback) as f64 / raw as f64).max(1.0);
        }
    }

    pub fn wanted(&self, cell: (i32, i32)) -> bool {
        let Some((bucket, _, _)) = self.window else {
            return true;
        };
        let owned = core(bucket, cell);
        let bounds = match self.bounds.get(&cell) {
            Some(Some(b)) => Rect {
                min_lon: f64::from(b.min[0]),
                min_lat: f64::from(b.min[1]),
                max_lon: f64::from(b.max[0]),
                max_lat: f64::from(b.max[1]),
            },
            Some(None) => owned,
            None => {
                let halo = (owned.max_lon - owned.min_lon) / 0.45;
                Rect {
                    min_lon: (owned.min_lon + owned.max_lon) * 0.5 - halo,
                    max_lon: (owned.min_lon + owned.max_lon) * 0.5 + halo,
                    min_lat: (owned.min_lat + owned.max_lat) * 0.5 - halo,
                    max_lat: (owned.min_lat + owned.max_lat) * 0.5 + halo,
                }
            }
        };
        self.view.is_none_or(|v| v.intersects(bounds))
    }
}

fn core(bucket: i32, (lat, lon): (i32, i32)) -> Rect {
    composition::core(
        ActiveBody::Earth,
        LocalTileId {
            zoom_bucket: bucket,
            lat_bucket: lat,
            lon_bucket: lon,
        },
    )
}

/// Also release the inactive body's data, and all globe data in local mode.
pub(crate) fn set_viewport(active: Option<ActiveBody>, view: Option<Viewport>) {
    for (body, slot) in [
        (ActiveBody::Earth, &GLOBE_CONTOUR_CACHE),
        (ActiveBody::Moon, &LUNAR_GLOBE_CONTOUR_CACHE),
        (ActiveBody::Mars, &MARS_GLOBE_CONTOUR_CACHE),
    ] {
        let cache = if active == Some(body) {
            Some(slot.get_or_init(Default::default))
        } else {
            slot.get()
        };
        if let Some(cache) = cache
            && let Ok(mut cache) = cache.lock()
        {
            if active == Some(body) {
                cache.residency.view = view;
            } else if cache.zoom_bucket != -1 {
                cache.reset_all();
                cache.residency.view = None;
            }
        }
    }
}

pub(super) fn prune(cache: &mut GlobeRegionCache) {
    let before = cache.tiles.len();
    let r = &mut cache.residency;
    r.costs.retain(|id, _| cache.tiles.contains_key(id));
    let visible: HashSet<_> = cache
        .tiles
        .keys()
        .copied()
        .filter(|&id| r.wanted(id))
        .collect();
    for id in &visible {
        if let Some(cost) = r.costs.get_mut(id) {
            cost.used = r.clock;
        }
    }
    let mut gpu: usize = r.costs.values().map(|c| c.gpu).sum();
    let mut cpu: usize = r.costs.values().map(|c| c.cpu).sum();
    let factor = r.gpu_factor.max(1.0);
    if gpu as f64 * factor + r.fallback_gpu as f64 <= r.gpu_budget as f64
        && cpu <= r.cpu_budget
        && cache.tiles.len() <= 4096
    {
        return;
    }
    let mut oldest: Vec<_> = r
        .costs
        .iter()
        .filter(|(id, _)| !visible.contains(id))
        .map(|(&id, c)| (c.used, id))
        .collect();
    oldest.sort();
    for (_, id) in oldest {
        if gpu as f64 * factor + r.fallback_gpu as f64 <= r.gpu_budget as f64
            && cpu <= r.cpu_budget
            && cache.tiles.len() <= 4096
        {
            break;
        }
        if let Some(cost) = r.costs.remove(&id) {
            gpu = gpu.saturating_sub(cost.gpu);
            cpu = cpu.saturating_sub(cost.cpu);
            cache.tiles.remove(&id);
            r.bounds.remove(&id);
        }
    }
    cache.order.retain(|id| cache.tiles.contains_key(id));
    if before != cache.tiles.len() {
        cache.mark_tiles_changed();
    }
}

/// Grow the source window when the fine grid cannot cover the screen, keeping
/// the requested grid at most 15x15. Broad views use the existing coarser tier.
pub(super) fn source_spec(view: Option<Viewport>, center: GeoPoint, zoom: f32) -> (f32, i32) {
    let fine = globe_zoom_to_tile_zoom(zoom);
    let Some(view) = view else {
        return (fine, 5);
    };
    for tile_zoom in [fine, 0.5] {
        let bucket = srtm_focus_cache::zoom_bucket_for_zoom(tile_zoom);
        let step = srtm_focus_cache::half_extent_for_zoom(tile_zoom) * 0.45;
        let cell = (
            (center.lat / step).round() as i32,
            (center.lon / step).round() as i32,
        );
        for radius in 5_i32..=7 {
            let outside = radius + 1;
            let edge_visible = (-outside..=outside).any(|offset| {
                [
                    (cell.0 + offset, cell.1 - outside),
                    (cell.0 + offset, cell.1 + outside),
                    (cell.0 - outside, cell.1 + offset),
                    (cell.0 + outside, cell.1 + offset),
                ]
                .into_iter()
                .any(|id| view.intersects(core(bucket, id)))
            });
            if !edge_visible {
                return (tile_zoom, radius);
            }
        }
    }
    (0.5, 7)
}

#[cfg(test)]
#[path = "globe_residency_tests.rs"]
mod tests;
