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
use super::srtm_stream;

const ELEVATION_OFFSET_M: f32 = 2.0;
const GEO_MARGIN_FACTOR: f32 = 0.75;
/// Bounds CPU tessellation & prevents wgpu's 256 MiB egui index-buffer ceiling (1kee-3p7).
const MAX_BUILDING_RENDER_POINTS: usize = 300_000;
static HAS_BUILDING_CACHE: AtomicBool = AtomicBool::new(false);

// ── Building footprint layer ──────────────────────────────────────────────────

struct ElevatedBuilding {
    bounds: [f32; 4], // [min_lon, min_lat, max_lon, max_lat]
    points: Vec<(GeoPoint, f32)>,
    is_polygon: bool,
}

impl ElevatedBuilding {
    fn from_polyline(poly: &LoadedPolyline, selected_root: Option<&Path>) -> Self {
        let points = crate::feature_heights::prepare(
            &poly.points,
            poly.elevations.as_deref(),
            usize::MAX,
            ELEVATION_OFFSET_M,
            |pt| srtm_stream::sample_elevation_m(selected_root, pt),
        );
        let mut min_lon = f32::INFINITY;
        let mut max_lon = f32::NEG_INFINITY;
        let mut min_lat = f32::INFINITY;
        let mut max_lat = f32::NEG_INFINITY;
        for &(p, _) in &points {
            min_lon = min_lon.min(p.lon);
            max_lon = max_lon.max(p.lon);
            min_lat = min_lat.min(p.lat);
            max_lat = max_lat.max(p.lat);
        }
        Self {
            bounds: [min_lon, min_lat, max_lon, max_lat],
            points,
            is_polygon: poly.is_polygon,
        }
    }
}

struct BuildingCache {
    data_gen: u64,
    last_root: std::path::PathBuf,
    covered_min_lat: f32,
    covered_max_lat: f32,
    covered_min_lon: f32,
    covered_max_lon: f32,
    features: Vec<ElevatedBuilding>,
}

struct BuildingCacheStore {
    cache: Option<Arc<BuildingCache>>,
    building: bool,
}

fn building_cache() -> &'static Mutex<BuildingCacheStore> {
    static CACHE: OnceLock<Mutex<BuildingCacheStore>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(BuildingCacheStore {
            cache: None,
            building: false,
        })
    })
}

pub(super) fn draw_buildings(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    render_zoom: f32,
    show_buildings: bool,
) {
    let _ = render_zoom; // zoom no longer drives cache invalidation

    if !show_buildings {
        if HAS_BUILDING_CACHE.load(Ordering::Relaxed)
            && let Ok(mut g) = building_cache().lock()
        {
            g.cache = None;
            HAS_BUILDING_CACHE.store(false, Ordering::Relaxed);
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

    let cache = {
        let mut store = match building_cache().lock() {
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

        if stale && !store.building {
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

            store.building = true;
            let snapshot = store.cache.clone();
            drop(store);

            let root_buf = root.to_path_buf();
            std::thread::spawn(move || {
                let polylines = load_features_from_cells(&root_buf, "building", load_bounds);
                let features = polylines
                    .iter()
                    .map(|p| ElevatedBuilding::from_polyline(p, Some(&root_buf)))
                    .collect();
                let next = Arc::new(BuildingCache {
                    data_gen: current_gen,
                    last_root: root_buf,
                    covered_min_lat,
                    covered_max_lat,
                    covered_min_lon,
                    covered_max_lon,
                    features,
                });

                let retired = if let Ok(mut store) = building_cache().lock() {
                    let old = store.cache.replace(next);
                    store.building = false;
                    HAS_BUILDING_CACHE.store(true, Ordering::Relaxed);
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

    let Some(cache) = cache else {
        return;
    };

    let params = local_projection_params(layout, view, viewport_center, extent_x_km, extent_y_km);
    let view_bounds = params
        .ground_bounds(painter.clip_rect().expand(8.0))
        .unwrap_or([-180.0, -90.0, 180.0, 90.0]);
    let span = layout.width.max(layout.height) * 8.0;

    let fill_color = theme::building_color();
    let thin_stroke = egui::epaint::PathStroke::new(0.8_f32, egui::Color32::from_rgb(100, 85, 65));
    let line_stroke = egui::Stroke::new(0.8_f32, fill_color);
    let mut rendered_points = 0usize;

    for feat in &cache.features {
        if feat.bounds[3] < view_bounds[1]
            || feat.bounds[1] > view_bounds[3]
            || feat.bounds[2] < view_bounds[0]
            || feat.bounds[0] > view_bounds[2]
        {
            continue;
        }
        if rendered_points >= MAX_BUILDING_RENDER_POINTS {
            break;
        }
        let mut pts: Vec<egui::Pos2> = Vec::with_capacity(feat.points.len());
        for &(pt, elev) in &feat.points {
            let (x, y, _) = params.project_logical(pt, elev);
            if x.is_finite() && y.is_finite() && x.abs() < span && y.abs() < span {
                pts.push(egui::pos2(x, y));
            }
        }

        if feat.is_polygon {
            if pts.len() < 3 {
                continue;
            }
            rendered_points += pts.len();
            painter.add(egui::Shape::Path(egui::epaint::PathShape {
                points: pts,
                closed: true,
                fill: fill_color,
                stroke: thin_stroke.clone(),
            }));
        } else if pts.len() >= 2 {
            rendered_points += pts.len();
            painter.add(egui::Shape::line(pts, line_stroke));
        }
    }
}
