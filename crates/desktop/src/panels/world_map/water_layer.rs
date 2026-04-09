use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest::{self, WaterPolyline};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::local_terrain_scene::{
    LocalLayout, local_geo_bounds, project_local, road_tile_zoom, visual_half_extent_for_zoom,
};
use super::srtm_stream;

// ── Water layer ────────────────────────────────────────────────────────────────
//
// Mirrors the road layer architecture: a static WaterCache holds pre-projected
// vertices so that `draw_water` is pure painter calls on every frame.

/// A water feature with elevation pre-sampled at every vertex.
struct ElevatedWater {
    points: Vec<(GeoPoint, f32)>, // (position, elevation_m)
    is_area: bool,
}

impl ElevatedWater {
    fn from_polyline(poly: &WaterPolyline, selected_root: Option<&Path>) -> Self {
        let pts = &poly.points;
        if pts.is_empty() {
            return Self {
                points: Vec::new(),
                is_area: poly.is_area,
            };
        }
        let points = pts
            .iter()
            .copied()
            .map(|pt| {
                let elevation =
                    srtm_stream::sample_elevation_m(selected_root, pt).unwrap_or(0.0) + 1.5;
                (pt, elevation)
            })
            .collect();
        Self {
            points,
            is_area: poly.is_area,
        }
    }
}

struct WaterCache {
    tile_zoom: u8,
    covered_min_lat: f32,
    covered_max_lat: f32,
    covered_min_lon: f32,
    covered_max_lon: f32,
    water_gen: u64,
    /// Elevation-enriched features, built off the render thread.
    features_elevated: Vec<ElevatedWater>,
}

struct WaterCacheStore {
    cache: Option<WaterCache>,
    building: Option<osm_ingest::GeoBounds>,
}

fn water_cache() -> &'static Mutex<WaterCacheStore> {
    static CACHE: OnceLock<Mutex<WaterCacheStore>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(WaterCacheStore {
            cache: None,
            building: None,
        })
    })
}

/// Clear the water tile cache so the next draw reloads from SQLite.
pub fn invalidate_water_cache() {
    if let Ok(mut g) = water_cache().lock() {
        g.cache = None;
    }
}

/// True while a background water-cache build is in progress.
pub fn water_cache_building_bounds() -> Option<osm_ingest::GeoBounds> {
    water_cache().lock().ok().and_then(|g| g.building)
}

pub(super) fn draw_water(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    render_zoom: f32,
    show_water: bool,
) {
    if !show_water {
        if let Ok(mut g) = water_cache().lock() {
            g.cache = None;
        }
        return;
    }

    let bounds = local_geo_bounds(viewport_center, view.local_zoom);
    let tile_zoom = road_tile_zoom(render_zoom);
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);
    let current_gen = osm_ingest::water_data_generation();

    // ── Stale check + background build launch ─────────────────────────────
    {
        let mut store = match water_cache().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let stale = store.cache.as_ref().map_or(true, |c| {
            c.tile_zoom != tile_zoom
                || c.water_gen != current_gen
                || bounds.min_lat < c.covered_min_lat
                || bounds.max_lat > c.covered_max_lat
                || bounds.min_lon < c.covered_min_lon
                || bounds.max_lon > c.covered_max_lon
        });

        if stale && store.building.is_none() {
            const GEO_MARGIN_FACTOR: f32 = 0.75;
            let lat_margin = (bounds.max_lat - bounds.min_lat) * GEO_MARGIN_FACTOR;
            let lon_margin = (bounds.max_lon - bounds.min_lon) * GEO_MARGIN_FACTOR;
            let load_bounds = osm_ingest::GeoBounds {
                min_lat: (bounds.min_lat - lat_margin).max(-85.0),
                max_lat: (bounds.max_lat + lat_margin).min(85.0),
                min_lon: (bounds.min_lon - lon_margin).max(-180.0),
                max_lon: (bounds.max_lon + lon_margin).min(180.0),
            };
            let (covered_min_lat, covered_max_lat) = (load_bounds.min_lat, load_bounds.max_lat);
            let (covered_min_lon, covered_max_lon) = (load_bounds.min_lon, load_bounds.max_lon);

            store.building = Some(load_bounds);
            drop(store);

            let root_buf = selected_root.map(|p| p.to_path_buf());
            std::thread::spawn(move || {
                let root_ref = root_buf.as_deref();
                let features_elevated =
                    osm_ingest::load_water_for_bounds(root_ref, load_bounds, tile_zoom)
                        .into_iter()
                        .map(|poly| ElevatedWater::from_polyline(&poly, root_ref))
                        .collect();

                if let Ok(mut store) = water_cache().lock() {
                    store.cache = Some(WaterCache {
                        tile_zoom,
                        covered_min_lat,
                        covered_max_lat,
                        covered_min_lon,
                        covered_max_lon,
                        water_gen: current_gen,
                        features_elevated,
                    });
                    store.building = None;
                }
                crate::app::request_repaint();
            });
        }
    }

    let mut store = match water_cache().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(cache) = &mut store.cache else {
        return;
    };
    let features = &cache.features_elevated;

    let water_col = crate::theme::water_color();
    let line_stroke = egui::Stroke::new(1.2, water_col);

    for feat in features {
        let mut pts = Vec::with_capacity(feat.points.len());
        for &(pt, elev) in &feat.points {
            let Some(pos) = project_local(
                layout,
                view,
                viewport_center,
                pt,
                elev,
                extent_x_km,
                extent_y_km,
            )
            .map(|p| p.pos) else {
                continue;
            };
            pts.push(pos);
        }

        if pts.len() < 2 {
            continue;
        }

        // For area features (lakes, reservoirs) close the ring so it draws as a
        // loop.  Do NOT use convex_polygon — OSM shorelines are non-convex and
        // the fan triangulation produces the sharp spike artifacts seen in the
        // screenshots.
        if feat.is_area && pts.len() >= 3 {
            pts.push(pts[0]);
        }
        painter.add(egui::Shape::line(pts, line_stroke));
    }
}
