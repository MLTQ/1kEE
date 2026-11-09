use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest;
use crate::theme;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::cell_loader::{LoadedPolyline, load_features_from_cells};
use super::local_terrain_scene::{LocalLayout, local_geo_bounds, visual_half_extent_for_zoom};
use super::srtm_stream;

#[path = "tree_geometry.rs"]
mod geometry;

const ELEVATION_OFFSET_M: f32 = 0.5;
const GEO_MARGIN_FACTOR: f32 = 0.75;

// ── Tree / forest polygon layer ────────────────────────────────────────────────

struct ElevatedTree {
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
        let fill = poly
            .is_polygon
            .then(|| geometry::tessellate(&points))
            .flatten();
        Self {
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
    cache: Option<TreeCache>,
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
    puffin::profile_function!();
    let _ = render_zoom; // zoom no longer drives cache invalidation

    if !show_trees {
        if let Ok(mut g) = tree_cache().lock() {
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
            drop(store);

            let root_buf = root.to_path_buf();
            std::thread::spawn(move || {
                let polylines = load_features_from_cells(&root_buf, "tree", load_bounds);
                let features = polylines
                    .iter()
                    .map(|p| ElevatedTree::from_polyline(p, Some(&root_buf)))
                    .collect();

                if let Ok(mut store) = tree_cache().lock() {
                    store.cache = Some(TreeCache {
                        data_gen: current_gen,
                        last_root: root_buf.clone(),
                        covered_min_lat,
                        covered_max_lat,
                        covered_min_lon,
                        covered_max_lon,
                        features,
                    });
                    store.building = false;
                }
                crate::app::request_repaint();
            });
        }
    }

    let store = match tree_cache().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(cache) = &store.cache else {
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
    paint_features(painter, &cache.features, &params, viewport_center);
}

fn paint_features(
    painter: &egui::Painter,
    features: &[ElevatedTree],
    params: &super::local_terrain_scene::projection::LocalProjectionParams,
    focus: GeoPoint,
) {
    let color = theme::tree_color();
    for feature in features {
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
            let mesh = geometry::project_fill(fill, color, painter.clip_rect(), project);
            if !mesh.is_empty() {
                painter.add(egui::Shape::mesh(mesh));
            }
        }
        let stroke = if feature.is_polygon {
            egui::Stroke::new(0.6, egui::Color32::from_rgb(20, 80, 30))
        } else {
            egui::Stroke::new(0.8, color)
        };
        painter.extend(geometry::outlines(&feature.points, stroke, project));
    }
}

#[cfg(test)]
#[path = "tree_layer_tests.rs"]
mod tests;
