use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest;
use crate::theme;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::cell_loader::{LoadedPolyline, load_features_from_cells};
use super::local_terrain_scene::{LocalLayout, local_geo_bounds, visual_half_extent_for_zoom};
use super::srtm_stream;

use super::tree_geometry as geometry;

const ELEVATION_OFFSET_M: f32 = 0.5;
const GEO_MARGIN_FACTOR: f32 = 0.75;
static HAS_TREE_CACHE: AtomicBool = AtomicBool::new(false);

// ── Tree / forest polygon layer ────────────────────────────────────────────────

struct ElevatedTree {
    bounds: [f32; 4], // [min_lon, min_lat, max_lon, max_lat]
    points: Vec<(GeoPoint, f32)>,
    is_polygon: bool,
    fill: Option<geometry::Fill>,
}

impl ElevatedTree {
    fn from_polyline(poly: &LoadedPolyline, selected_root: Option<&Path>) -> Self {
        let mut points = crate::feature_heights::prepare(
            &poly.points,
            poly.elevations.as_deref(),
            usize::MAX,
            ELEVATION_OFFSET_M,
            |pt| srtm_stream::sample_elevation_m(selected_root, pt),
        );
        geometry::unwrap(&mut points);
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
        let fill = poly
            .is_polygon
            .then(|| geometry::tessellate(&points))
            .flatten();
        Self {
            bounds: [min_lon, min_lat, max_lon, max_lat],
            points,
            is_polygon: poly.is_polygon,
            fill,
        }
    }
}

struct TreeCache {
    data_gen: u64,
    last_root: std::path::PathBuf,
    covered_min_lat: f32,
    covered_max_lat: f32,
    covered_min_lon: f32,
    covered_max_lon: f32,
    features: Vec<ElevatedTree>,
}

struct TreeCacheStore {
    cache: Option<Arc<TreeCache>>,
    building: bool,
}

fn tree_cache() -> &'static Mutex<TreeCacheStore> {
    static CACHE: OnceLock<Mutex<TreeCacheStore>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(TreeCacheStore {
            cache: None,
            building: false,
        })
    })
}

pub(super) fn draw_trees(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    render_zoom: f32,
    show_trees: bool,
) {
    let _ = render_zoom; // zoom no longer drives cache invalidation

    if !show_trees {
        if HAS_TREE_CACHE.load(Ordering::Relaxed)
            && let Ok(mut g) = tree_cache().lock()
        {
            g.cache = None;
            HAS_TREE_CACHE.store(false, Ordering::Relaxed);
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
        let mut store = match tree_cache().lock() {
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
                let polylines = load_features_from_cells(&root_buf, "tree", load_bounds);
                let features = polylines
                    .iter()
                    .map(|p| ElevatedTree::from_polyline(p, Some(&root_buf)))
                    .collect();
                let next = Arc::new(TreeCache {
                    data_gen: current_gen,
                    last_root: root_buf,
                    covered_min_lat,
                    covered_max_lat,
                    covered_min_lon,
                    covered_max_lon,
                    features,
                });

                let retired = if let Ok(mut store) = tree_cache().lock() {
                    let old = store.cache.replace(next);
                    store.building = false;
                    HAS_TREE_CACHE.store(true, Ordering::Relaxed);
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

    // A cache from another root can finish while the source selection changes.
    if cache.last_root != root {
        return;
    }
    let params = super::local_terrain_scene::projection::local_projection_params(
        layout,
        view,
        viewport_center,
        extent_x_km,
        extent_y_km,
    );
    let view_bounds = params
        .ground_bounds(painter.clip_rect().expand(8.0))
        .unwrap_or([-180.0, -90.0, 180.0, 90.0]);
    paint_features(painter, &cache.features, &params, viewport_center, view_bounds);
}

#[inline]
fn bbox_intersects(a: [f32; 4], b: [f32; 4]) -> bool {
    if a[3] < b[1] || a[1] > b[3] {
        return false;
    }
    (a[2] >= b[0] && a[0] <= b[2])
        || (a[2] + 360.0 >= b[0] && a[0] + 360.0 <= b[2])
        || (a[2] - 360.0 >= b[0] && a[0] - 360.0 <= b[2])
}

fn paint_features(
    painter: &egui::Painter,
    features: &[ElevatedTree],
    params: &super::local_terrain_scene::projection::LocalProjectionParams,
    focus: GeoPoint,
    view_bounds: [f32; 4],
) {
    let color = theme::tree_color();
    let poly_stroke = egui::Stroke::new(0.6_f32, egui::Color32::from_rgb(20, 80, 30));
    let line_stroke = egui::Stroke::new(0.8_f32, color);
    let clip = painter.clip_rect();

    // Batch all visible tree fills into one mesh instead of one mesh per forest polygon.
    let mut fill_mesh = egui::Mesh::default();
    let mut pos_scratch = Vec::new();
    let mut outline_shapes = Vec::new();

    for feature in features {
        if !bbox_intersects(feature.bounds, view_bounds) {
            continue;
        }
        let Some((anchor, _)) = feature
            .points
            .iter()
            .find(|(p, _)| p.lon.is_finite() && p.lat.is_finite())
        else {
            continue;
        };
        let shift = ((focus.lon - anchor.lon) / 360.0).round() * 360.0;
        let project = |mut p: GeoPoint, h| {
            p.lon += shift;
            let (x, y, _) = params.project_logical(p, h);
            let p = egui::pos2(x, y);
            p.is_finite().then_some(p)
        };
        if let Some(fill) = &feature.fill {
            geometry::append_fill(fill, color, clip, project, &mut fill_mesh, &mut pos_scratch);
        }
        let stroke = if feature.is_polygon {
            poly_stroke
        } else {
            line_stroke
        };
        outline_shapes.extend(geometry::outlines(&feature.points, stroke, project));
    }
    if !fill_mesh.is_empty() {
        painter.add(egui::Shape::mesh(fill_mesh));
    }
    if !outline_shapes.is_empty() {
        painter.extend(outline_shapes);
    }
}

#[cfg(test)]
#[path = "tree_layer_tests.rs"]
mod tests;
