//! View-dependent residency; retain bounds, not offscreen contour geometry.
use super::*;
use crate::model::ActiveBody;
use crate::panels::world_map::local_terrain_scene::projection::LocalProjectionParams;

#[derive(Clone, Copy, Debug)]
pub(super) struct Bounds {
    min: [f32; 3], // longitude, latitude, elevation
    max: [f32; 3],
}

impl Bounds {
    /// Computed on the reader, never by scanning geometry during paint.
    pub(super) fn from_contours(contours: &[ContourPath]) -> Option<Self> {
        let mut bounds = Self {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        };
        for contour in contours {
            for point in &contour.points {
                let p = [point.lon, point.lat, contour.elevation_m];
                if p.iter().all(|v| v.is_finite()) {
                    for (i, value) in p.into_iter().enumerate() {
                        bounds.min[i] = bounds.min[i].min(value);
                        bounds.max[i] = bounds.max[i].max(value);
                    }
                }
            }
        }
        bounds.min[0].is_finite().then_some(bounds)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Viewport {
    projection: LocalProjectionParams,
    rect: egui::Rect,
    body: ActiveBody,
}

impl Viewport {
    fn intersects(self, bounds: Bounds) -> bool {
        // The local transform is affine. The eight projected box corners
        // enclose every contour, including elevated terrain and crossing lines.
        let points: [egui::Pos2; 8] = std::array::from_fn(|i| {
            let p: [f32; 3] = std::array::from_fn(|axis| {
                if i & (1 << axis) == 0 {
                    bounds.min[axis]
                } else {
                    bounds.max[axis]
                }
            });
            let (x, y, _) = self.projection.project_logical(
                GeoPoint {
                    lon: p[0],
                    lat: p[1],
                },
                p[2],
            );
            egui::pos2(x, y)
        });
        if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return true; // Never cull on an invalid camera transform.
        }
        let corners = [
            self.rect.left_top(),
            self.rect.right_top(),
            self.rect.left_bottom(),
            self.rect.right_bottom(),
        ];
        let edges = [
            points[1] - points[0],
            points[2] - points[0],
            points[4] - points[0],
        ];
        // Separating axes of the projected box and screen rectangle. Unlike
        // a screen AABB alone, this also rejects rotated offscreen corners.
        [egui::vec2(1.0, 0.0), egui::vec2(0.0, 1.0)]
            .into_iter()
            .chain(edges.map(|e| egui::vec2(-e.y, e.x)))
            .all(|axis| {
                let range = |ps: &[egui::Pos2]| {
                    ps.iter()
                        .map(|p| p.to_vec2().dot(axis))
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                            (lo.min(v), hi.max(v))
                        })
                };
                let (a, b) = range(&points);
                let (c, d) = range(&corners);
                a <= d && c <= b
            })
    }

    fn footprint(self, key: &CacheKey, empty: bool) -> Bounds {
        let zooms = [0.5, 1.5, 2.5, 3.5, 5.0, 8.0, 12.0, 16.0, 25.0, 35.0, 50.0];
        let zoom = zooms[key.zoom_bucket.clamp(0, 10) as usize];
        let spec = match self.body {
            ActiveBody::Earth => srtm_focus_cache::zoom::spec_for_zoom(zoom),
            ActiveBody::Moon => srtm_focus_cache::zoom::lunar_spec_for_zoom(zoom),
            ActiveBody::Mars => srtm_focus_cache::zoom::mars_spec_for_zoom(zoom),
        };
        let step = spec.half_extent_deg * 0.45;
        // Unknown legacy tiles can extend far beyond their core. Only use the
        // small core for a decoded empty tile, whose footprint tracks coverage.
        let half = if empty {
            step * 0.5
        } else {
            spec.half_extent_deg
        };
        let lon = key.lon_bucket as f32 * step;
        let lat = key.lat_bucket as f32 * step;
        Bounds {
            min: [lon - half, lat - half, if empty { 0.0 } else { -12_000.0 }],
            max: [lon + half, lat + half, if empty { 0.0 } else { 25_000.0 }],
        }
    }
}

#[derive(Default)]
pub(super) struct Residency {
    viewport: Option<Viewport>,
    window: Option<(i32, i32, i32)>,
    // Small metadata survives eviction so a stationary view doesn't repeatedly
    // decode an offscreen tile. None means decoded empty, not a missing tile.
    bounds: HashMap<CacheKey, Option<Bounds>>,
}

impl Residency {
    pub(super) fn wanted(&self, key: &CacheKey) -> bool {
        if self
            .window
            .is_some_and(|(lat, lon, radius)| local_tile_distance(key, lat, lon) > radius)
        {
            return false;
        }
        let Some(view) = self.viewport else {
            return true;
        };
        let bounds = match self.bounds.get(key) {
            Some(Some(bounds)) => *bounds,
            known => view.footprint(key, known.is_some()),
        };
        view.intersects(bounds)
    }

    pub(super) fn record(&mut self, key: CacheKey, bounds: Option<Bounds>) {
        self.bounds.insert(key, bounds);
    }

    pub(super) fn clear_geometry_metadata(&mut self) {
        self.bounds.clear();
        self.window = None;
    }

    pub(super) fn set_window(&mut self, lat: i32, lon: i32, radius: i32) -> bool {
        if self.window == Some((lat, lon, radius)) {
            return false;
        }
        self.window = Some((lat, lon, radius));
        self.bounds
            .retain(|key, _| local_tile_distance(key, lat, lon) <= radius + 3);
        true
    }
}

/// Update both Earth source tiers, including an inactive base fallback cache.
/// Other bodies' local caches are released when switching planets.
pub(crate) fn set_local_viewport(
    body: ActiveBody,
    projection: LocalProjectionParams,
    rect: egui::Rect,
) {
    let viewport = Viewport {
        body,
        projection,
        rect: rect.expand(8.0),
    };
    for (cache_body, slot) in [
        (ActiveBody::Earth, &LOCAL_CONTOUR_CACHE),
        (ActiveBody::Earth, &EARTH_BASE_CONTOUR_CACHE),
        (ActiveBody::Moon, &LUNAR_LOCAL_CONTOUR_CACHE),
        (ActiveBody::Mars, &MARS_LOCAL_CONTOUR_CACHE),
    ] {
        // Initialize active caches before the loader sets their scene state.
        let cache = if cache_body == body {
            Some(slot.get_or_init(|| Mutex::new(LocalRegionCache::default())))
        } else {
            slot.get()
        };
        if let Some(cache) = cache
            && let Ok(mut cache) = cache.lock()
        {
            if cache_body == body {
                cache.residency.viewport = Some(viewport);
                prune(&mut cache);
            } else if cache.scene_key.is_some() {
                cache.reset_all();
            }
        }
    }
}

/// Drop local residency when leaving the local scene. Globe transition loads
/// without a local viewport are left alone on subsequent frames.
pub(crate) fn leave_local_view() {
    for slot in [
        &LOCAL_CONTOUR_CACHE,
        &EARTH_BASE_CONTOUR_CACHE,
        &LUNAR_LOCAL_CONTOUR_CACHE,
        &MARS_LOCAL_CONTOUR_CACHE,
    ] {
        if let Some(cache) = slot.get()
            && let Ok(mut cache) = cache.lock()
            && cache.residency.viewport.is_some()
        {
            cache.reset_all();
        }
    }
    super::super::local_contour_pass::clear_instances();
}

pub(super) fn prune(cache: &mut LocalRegionCache) {
    let before = cache.entries.len();
    cache.entries.retain(|key, _| cache.residency.wanted(key));
    cache
        .read_progress
        .retain(|key, _| cache.residency.wanted(key));
    cache
        .published_tiles
        .retain(|key| cache.residency.wanted(key));
    if cache.entries.len() != before {
        cache.mark_entries_changed();
        // A completed replacement merge releases the previous visible snapshot.
        // Empty views have no replacement worker, so release immediately.
        if cache.entries.is_empty() {
            cache.merged = None;
            cache.zoom_fallback = None;
        }
    }
}

#[cfg(test)]
#[path = "contour_residency_tests.rs"]
mod tests;
