/// Power infrastructure layer renderer.
///
/// Voltage-tier LOD: at low zoom only ultra-high-voltage lines are shown;
/// as zoom increases, progressively more tiers appear.  This maps the POWR
/// class byte directly to visual priority without extra data.
use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest;
use crate::theme;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::cell_loader::{LoadedPolyline, load_features_from_cells};
use super::local_terrain_scene::{
    LocalLayout, local_geo_bounds, project_local, visual_half_extent_for_zoom,
};

const GEO_MARGIN_FACTOR: f32 = 0.75;

// ── LOD thresholds (local_zoom values) ───────────────────────────────────────
// local_zoom ~ 1.0 is regional (≈200 km), ~ 6.0 is city-level (≈5 km)
const ZOOM_SHOW_HIGH: f32 = 1.5;   // 100-299 kV visible from here
const ZOOM_SHOW_MED: f32 = 3.0;    // 50-99 kV
const ZOOM_SHOW_LOW: f32 = 5.0;    // <50 kV
const ZOOM_SHOW_MINOR: f32 = 7.0;  // minor/service lines

// ── Cache ─────────────────────────────────────────────────────────────────────

struct PowerCache {
    data_gen: u64,
    last_root: std::path::PathBuf,
    covered_min_lat: f32,
    covered_max_lat: f32,
    covered_min_lon: f32,
    covered_max_lon: f32,
    features: Vec<LoadedPolyline>,
}

struct PowerCacheStore {
    cache: Option<PowerCache>,
    building: Option<osm_ingest::GeoBounds>,
}

fn power_cache() -> &'static Mutex<PowerCacheStore> {
    static C: OnceLock<Mutex<PowerCacheStore>> = OnceLock::new();
    C.get_or_init(|| {
        Mutex::new(PowerCacheStore {
            cache: None,
            building: None,
        })
    })
}

pub fn power_cache_building_bounds() -> Option<osm_ingest::GeoBounds> {
    power_cache().lock().ok().and_then(|g| g.building)
}

// ── Render ────────────────────────────────────────────────────────────────────

pub(super) fn draw_power(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    show: bool,
) {
    puffin::profile_function!();

    if !show {
        if let Ok(mut g) = power_cache().lock() {
            g.cache = None;
        }
        return;
    }

    let Some(root) = selected_root else { return };

    let bounds = local_geo_bounds(viewport_center, view.local_zoom);
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);
    let current_gen = osm_ingest::road_data_generation();
    let zoom = view.local_zoom;

    {
        let mut store = match power_cache().lock() {
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
                let features = load_features_from_cells(&root_buf, "power", load_bounds);
                if let Ok(mut store) = power_cache().lock() {
                    store.cache = Some(PowerCache {
                        data_gen: current_gen,
                        last_root: root_buf,
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

    let store = match power_cache().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(cache) = &store.cache else { return };

    for feat in &cache.features {
        // Voltage LOD: skip tiers that are below the current zoom level
        let visible = match feat.class.as_str() {
            "line_ultra" | "substation" | "power_plant" => true,
            "line_high" => zoom >= ZOOM_SHOW_HIGH,
            "line_med" => zoom >= ZOOM_SHOW_MED,
            "line_low" => zoom >= ZOOM_SHOW_LOW,
            "minor_line" | "tower" => zoom >= ZOOM_SHOW_MINOR,
            _ => true,
        };
        if !visible {
            continue;
        }

        let pts: Vec<egui::Pos2> = feat
            .points
            .iter()
            .filter_map(|&pt| {
                project_local(layout, view, viewport_center, pt, 0.0, extent_x_km, extent_y_km)
                    .map(|p| p.pos)
            })
            .collect();

        if pts.len() < 2 {
            continue;
        }

        match feat.class.as_str() {
            "substation" | "power_plant" => {
                let fill = theme::power_area_color();
                let stroke = egui::Stroke::new(1.0, theme::power_ultra_color());
                painter.add(egui::Shape::convex_polygon(pts, fill, stroke));
            }
            cls => {
                let (color, width) = match cls {
                    "line_ultra" => (theme::power_ultra_color(), 2.2),
                    "line_high" => (theme::power_high_color(), 1.8),
                    "line_med" => (theme::power_med_color(), 1.3),
                    "line_low" => (theme::power_low_color(), 1.0),
                    _ => (theme::power_minor_color(), 0.7),
                };
                painter.add(egui::Shape::line(pts, egui::Stroke::new(width, color)));
            }
        }
    }
}
