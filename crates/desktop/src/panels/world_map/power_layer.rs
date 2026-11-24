/// Power infrastructure layer renderer.
///
/// Voltage-tier LOD: at low zoom only ultra-high-voltage lines are shown;
/// as zoom increases, progressively more tiers appear.  This maps the POWR
/// class byte directly to visual priority without extra data.
use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest;
use crate::theme;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::cell_loader::{LoadedPolyline, load_features_from_cells};
use super::local_terrain_scene::{
    LocalLayout, local_geo_bounds, projection::local_projection_params, visual_half_extent_for_zoom,
};

const GEO_MARGIN_FACTOR: f32 = 0.75;
static HAS_POWER_CACHE: AtomicBool = AtomicBool::new(false);

// ── LOD thresholds (local_zoom values) ───────────────────────────────────────
const ZOOM_SHOW_HIGH: f32 = 1.5;   // 100-299 kV visible from here
const ZOOM_SHOW_MED: f32 = 3.0;    // 50-99 kV
const ZOOM_SHOW_LOW: f32 = 5.0;    // <50 kV
const ZOOM_SHOW_MINOR: f32 = 7.0;  // minor/service lines

// ── Cache ─────────────────────────────────────────────────────────────────────

struct PowerFeature {
    bounds: [f32; 4],
    polyline: LoadedPolyline,
}

impl PowerFeature {
    fn new(polyline: LoadedPolyline) -> Self {
        let mut min_lon = f32::INFINITY;
        let mut max_lon = f32::NEG_INFINITY;
        let mut min_lat = f32::INFINITY;
        let mut max_lat = f32::NEG_INFINITY;
        for &p in &polyline.points {
            min_lon = min_lon.min(p.lon);
            max_lon = max_lon.max(p.lon);
            min_lat = min_lat.min(p.lat);
            max_lat = max_lat.max(p.lat);
        }
        Self {
            bounds: [min_lon, min_lat, max_lon, max_lat],
            polyline,
        }
    }
}

struct PowerCache {
    data_gen: u64,
    last_root: std::path::PathBuf,
    covered_min_lat: f32,
    covered_max_lat: f32,
    covered_min_lon: f32,
    covered_max_lon: f32,
    features: Vec<PowerFeature>,
}

struct PowerCacheStore {
    cache: Option<Arc<PowerCache>>,
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
    if !show {
        if HAS_POWER_CACHE.load(Ordering::Relaxed)
            && let Ok(mut g) = power_cache().lock()
        {
            g.cache = None;
            HAS_POWER_CACHE.store(false, Ordering::Relaxed);
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

    let cache = {
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
            let snapshot = store.cache.clone();
            drop(store);

            let root_buf = root.to_path_buf();
            std::thread::spawn(move || {
                let features = load_features_from_cells(&root_buf, "power", load_bounds)
                    .into_iter()
                    .map(PowerFeature::new)
                    .collect();
                let next = Arc::new(PowerCache {
                    data_gen: current_gen,
                    last_root: root_buf,
                    covered_min_lat,
                    covered_max_lat,
                    covered_min_lon,
                    covered_max_lon,
                    features,
                });
                let retired = if let Ok(mut store) = power_cache().lock() {
                    let old = store.cache.replace(next);
                    store.building = None;
                    HAS_POWER_CACHE.store(true, Ordering::Relaxed);
                    old
                } else {
                    None
                };
                drop(retired);
                crate::app::request_repaint();
            });
            snapshot
        } else {
            store.cache.clone()
        }
    };

    let Some(cache) = cache else { return };

    let params = local_projection_params(layout, view, viewport_center, extent_x_km, extent_y_km);
    let view_bounds = params
        .ground_bounds(painter.clip_rect().expand(8.0))
        .unwrap_or([-180.0, -90.0, 180.0, 90.0]);
    let span = layout.width.max(layout.height) * 8.0;

    let c_area = theme::power_area_color();
    let c_ultra = theme::power_ultra_color();
    let c_high = theme::power_high_color();
    let c_med = theme::power_med_color();
    let c_low = theme::power_low_color();
    let c_minor = theme::power_minor_color();
    let area_stroke = egui::Stroke::new(1.0_f32, c_ultra);

    for item in &cache.features {
        if item.bounds[3] < view_bounds[1]
            || item.bounds[1] > view_bounds[3]
            || item.bounds[2] < view_bounds[0]
            || item.bounds[0] > view_bounds[2]
        {
            continue;
        }
        let feat = &item.polyline;
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

        let mut pts = Vec::with_capacity(feat.points.len());
        for &pt in &feat.points {
            let (x, y, _) = params.project_logical(pt, 0.0);
            if x.is_finite() && y.is_finite() && x.abs() < span && y.abs() < span {
                pts.push(egui::pos2(x, y));
            }
        }

        if pts.len() < 2 {
            continue;
        }

        match feat.class.as_str() {
            "substation" | "power_plant" => {
                painter.add(egui::Shape::convex_polygon(pts, c_area, area_stroke));
            }
            cls => {
                let (color, width) = match cls {
                    "line_ultra" => (c_ultra, 2.2_f32),
                    "line_high" => (c_high, 1.8_f32),
                    "line_med" => (c_med, 1.3_f32),
                    "line_low" => (c_low, 1.0_f32),
                    _ => (c_minor, 0.7_f32),
                };
                painter.add(egui::Shape::line(pts, egui::Stroke::new(width, color)));
            }
        }
    }
}
