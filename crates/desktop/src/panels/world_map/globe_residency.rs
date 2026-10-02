//! Conservative globe frustum culling and bounded source/output ownership.
use super::*;
use crate::model::{ActiveBody, GlobeViewState};
use crate::panels::world_map::{
    globe_scene::GlobeLayout, local_contour_pass::LocalTileId, local_terrain_scene::composition,
};
use tile_archive::contour_grid::Bounds as Rect;

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
        }
        self.window = Some(window);
        self.bounds
            .retain(|&(lat, lon), _| (lat - center.0).abs() <= 11 && (lon - center.1).abs() <= 11);
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
        self.bounds.insert(cell, bounds);
    }

    pub fn wanted(&self, cell: (i32, i32)) -> bool {
        let Some((bucket, lat, lon)) = self.window else {
            return true;
        };
        if (cell.0 - lat).abs() > 8 || (cell.1 - lon).abs() > 8 {
            return false;
        }
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
    cache.tiles.retain(|&id, _| cache.residency.wanted(id));
    cache.order.retain(|id| cache.tiles.contains_key(id));
    if before != cache.tiles.len() {
        cache.mark_tiles_changed();
    }
    if cache.residency.cells.is_empty() {
        cache.merged = None;
        cache.zoom_fallback = None;
    }
}

#[cfg(test)]
#[path = "globe_residency_tests.rs"]
mod tests;
