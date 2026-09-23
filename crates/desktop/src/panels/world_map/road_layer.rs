use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest::{self, RoadLayerKind};
use crate::theme;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::local_contour_pass::{LocalContourCallback, LocalContourPass};
use super::local_terrain_scene::{
    LocalLayout, local_geo_bounds, projection, visual_half_extent_for_zoom,
};

#[path = "road_geometry.rs"]
mod geometry;
use geometry::RoadGeometry;

const GEO_MARGIN_FACTOR: f32 = 0.75;

/// Explicit data/terrain resets invalidate roads; visibility toggles retain them.
pub fn invalidate_road_cache() {
    let old = if let Ok(mut store) = road_cache().lock() {
        store.epoch = store.epoch.wrapping_add(1);
        store.cache.take()
    } else {
        None
    };
    // Releasing large geometry must not hold the publication lock or UI thread.
    if let Some(old) = old {
        std::thread::spawn(move || drop(old));
    }
}

pub fn road_cache_building_bounds() -> Option<osm_ingest::GeoBounds> {
    road_cache().try_lock().ok().and_then(|g| g.building)
}

struct RoadCache {
    road_gen: u64,
    root: Option<PathBuf>,
    bounds: osm_ingest::GeoBounds,
    colors: [egui::Color32; 2],
    geometry: RoadGeometry,
}

#[derive(Default)]
struct RoadCacheStore {
    cache: Option<Arc<RoadCache>>,
    building: Option<osm_ingest::GeoBounds>,
    epoch: u64,
}

fn road_cache() -> &'static Mutex<RoadCacheStore> {
    static CACHE: OnceLock<Mutex<RoadCacheStore>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(RoadCacheStore::default()))
}

fn covers(outer: osm_ingest::GeoBounds, inner: osm_ingest::GeoBounds) -> bool {
    outer.min_lat <= inner.min_lat
        && outer.max_lat >= inner.max_lat
        && outer.min_lon <= inner.min_lon
        && outer.max_lon >= inner.max_lon
}

pub(super) fn draw_roads(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    render_zoom: f32,
    show_major_roads: bool,
    show_minor_roads: bool,
) {
    puffin::profile_function!();
    if !show_major_roads && !show_minor_roads {
        return;
    }
    let bounds = local_geo_bounds(viewport_center, view.local_zoom);
    let current_gen = osm_ingest::road_data_generation();
    let colors = [theme::road_major_color(), theme::road_minor_color()];
    let cache = {
        let Ok(mut store) = road_cache().try_lock() else {
            painter.ctx().request_repaint();
            return;
        };
        let stale = store.cache.as_ref().is_none_or(|c| {
            c.road_gen != current_gen
                || c.root.as_deref() != selected_root
                || c.colors != colors
                || !covers(c.bounds, bounds)
        });
        if stale && store.building.is_none() {
            let lat_margin = (bounds.max_lat - bounds.min_lat) * GEO_MARGIN_FACTOR;
            let lon_margin = (bounds.max_lon - bounds.min_lon) * GEO_MARGIN_FACTOR;
            let load_bounds = osm_ingest::GeoBounds {
                min_lat: (bounds.min_lat - lat_margin).max(-85.0),
                max_lat: (bounds.max_lat + lat_margin).min(85.0),
                min_lon: (bounds.min_lon - lon_margin).max(-180.0),
                max_lon: (bounds.max_lon + lon_margin).min(180.0),
            };
            store.building = Some(load_bounds);
            let epoch = store.epoch;
            let root = selected_root.map(Path::to_owned);
            let zoom = super::local_terrain_scene::road_tile_zoom(render_zoom);
            let ctx = painter.ctx().clone();
            let spawned = std::thread::Builder::new()
                .name("road-geometry".into())
                .spawn(move || {
                    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        static VERSION: AtomicU64 = AtomicU64::new(1);
                        let version = VERSION.fetch_add(1, Ordering::Relaxed);
                        let roads = osm_ingest::load_roads_for_bounds(
                            root.as_deref(),
                            load_bounds,
                            zoom,
                            RoadLayerKind::All,
                        );
                        let geometry = RoadGeometry::build(roads, root.as_deref(), version, colors);
                        Arc::new(RoadCache {
                            road_gen: current_gen,
                            root,
                            bounds: load_bounds,
                            colors,
                            geometry,
                        })
                    }));
                    let mut retired = None;
                    if let Ok(mut store) = road_cache().lock() {
                        store.building = None;
                        match built {
                            Ok(cache) => {
                                retired = if store.epoch == epoch {
                                    store.cache.replace(cache)
                                } else {
                                    Some(cache)
                                };
                            }
                            Err(_) => eprintln!("[1kEE] road geometry worker failed; retrying"),
                        }
                    }
                    drop(retired);
                    ctx.request_repaint();
                });
            if let Err(error) = spawned {
                store.building = None;
                eprintln!("[1kEE] cannot start road geometry worker: {error}");
            }
        }
        // Only clone Arc-backed batches after releasing the shared lock.
        store
            .cache
            .as_ref()
            .filter(|c| c.root.as_deref() == selected_root)
            .cloned()
    };
    let Some(cache) = cache else {
        return;
    };
    let mut batches = Vec::new();
    if show_major_roads {
        batches.extend(cache.geometry.major.iter().cloned());
    }
    if show_minor_roads {
        batches.extend(cache.geometry.minor.iter().cloned());
    }
    if batches.is_empty() {
        return;
    }
    let half = visual_half_extent_for_zoom(view.local_zoom);
    let extent_x = (half * 111.32 * viewport_center.lat.to_radians().cos().abs().max(0.2)).max(1.0);
    let extent_y = (half * 111.32).max(1.0);
    let params =
        projection::local_projection_params(layout, view, viewport_center, extent_x, extent_y);
    let ppp = painter.ctx().pixels_per_point();
    painter.add(
        LocalContourCallback::new(
            LocalContourPass::Roads,
            batches,
            &params,
            1.0,
            0.8 * ppp,
            1.35 * ppp,
            painter.ctx().clone(),
        )
        .into_paint_callback(painter.clip_rect()),
    );
}
