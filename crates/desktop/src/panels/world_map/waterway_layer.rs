use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest;
use crate::theme;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::cell_loader::{LoadedPolyline, load_features_from_cells};
use super::local_terrain_scene::{
    LocalLayout, local_geo_bounds, project_local, visual_half_extent_for_zoom,
};
use super::srtm_stream;

const ELEVATION_OFFSET_M: f32 = 2.0;
const GEO_MARGIN_FACTOR: f32 = 0.75;

// ── Waterway layer ─────────────────────────────────────────────────────────────

struct ElevatedWaterway {
    points: Vec<(GeoPoint, f32)>,
}

impl ElevatedWaterway {
    fn from_polyline(poly: &LoadedPolyline, selected_root: Option<&Path>) -> Self {
        let points = poly
            .points
            .iter()
            .copied()
            .map(|pt| {
                let elev = srtm_stream::sample_elevation_m(selected_root, pt).unwrap_or(0.0)
                    + ELEVATION_OFFSET_M;
                (pt, elev)
            })
            .collect();
        Self { points }
    }
}

struct WaterwayCache {
    data_gen: u64,
    last_root: std::path::PathBuf,
    covered_min_lat: f32,
    covered_max_lat: f32,
    covered_min_lon: f32,
    covered_max_lon: f32,
    features: Vec<ElevatedWaterway>,
}

struct WaterwayCacheStore {
    cache: Option<WaterwayCache>,
    building: Option<osm_ingest::GeoBounds>,
}

fn waterway_cache() -> &'static Mutex<WaterwayCacheStore> {
    static CACHE: OnceLock<Mutex<WaterwayCacheStore>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(WaterwayCacheStore {
            cache: None,
            building: None,
        })
    })
}

/// The bounding box of the background waterway-cache build in progress, if any.
pub fn waterway_cache_building_bounds() -> Option<osm_ingest::GeoBounds> {
    waterway_cache().lock().ok().and_then(|g| g.building)
}

pub(super) fn draw_waterways(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    render_zoom: f32,
    show_waterways: bool,
) {
    puffin::profile_function!();
    let _ = render_zoom; // zoom no longer drives cache invalidation

    if !show_waterways {
        if let Ok(mut g) = waterway_cache().lock() {
            g.cache = None;
        }
        return;
    }

    let Some(root) = selected_root else {
        return;
    };

    let bounds = local_geo_bounds(viewport_center, view.local_zoom);
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);
    let current_gen = osm_ingest::road_data_generation();

    {
        let mut store = match waterway_cache().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let stale = store.cache.as_ref().map_or(true, |c| {
            c.data_gen != current_gen
                || c.last_root != *root
                || bounds.min_lat < c.covered_min_lat
                || bounds.max_lat > c.covered_max_lat
                || bounds.min_lon < c.covered_min_lon
                || bounds.max_lon > c.covered_max_lon
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
            let (covered_min_lat, covered_max_lat) = (load_bounds.min_lat, load_bounds.max_lat);
            let (covered_min_lon, covered_max_lon) = (load_bounds.min_lon, load_bounds.max_lon);

            store.building = Some(load_bounds);
            drop(store);

            let root_buf = root.to_path_buf();
            std::thread::spawn(move || {
                let polylines = load_features_from_cells(&root_buf, "waterway", load_bounds);
                let features = polylines
                    .iter()
                    .map(|p| ElevatedWaterway::from_polyline(p, Some(&root_buf)))
                    .collect();

                if let Ok(mut store) = waterway_cache().lock() {
                    store.cache = Some(WaterwayCache {
                        data_gen: current_gen,
                        last_root: root_buf.clone(),
                        covered_min_lat,
                        covered_max_lat,
                        covered_min_lon,
                        covered_max_lon,
                        features,
                    });
                    store.building = None;
                }
                crate::app::request_repaint();
            });
        }
    }

    let store = match waterway_cache().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(cache) = &store.cache else {
        return;
    };

    let stroke = egui::Stroke::new(1.2, theme::waterway_color());

    for feat in &cache.features {
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
        if pts.len() >= 2 {
            painter.add(egui::Shape::line(pts, stroke));
        }
    }
}
