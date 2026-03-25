use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest;
use crate::theme;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::cell_loader::{LoadedPolyline, load_features_from_cells};
use super::local_terrain_scene::{
    LocalLayout, local_geo_bounds, project_local, road_tile_zoom, visual_half_extent_for_zoom,
};
use super::srtm_stream;

const ELEVATION_OFFSET_M: f32 = 0.5;

// ── Tree / forest polygon layer ────────────────────────────────────────────────

struct ElevatedTree {
    points: Vec<(GeoPoint, f32)>,
    is_polygon: bool,
}

impl ElevatedTree {
    fn from_polyline(poly: &LoadedPolyline, selected_root: Option<&Path>) -> Self {
        let points = poly
            .points
            .iter()
            .copied()
            .map(|pt| {
                let elev =
                    srtm_stream::sample_elevation_m(selected_root, pt).unwrap_or(0.0)
                        + ELEVATION_OFFSET_M;
                (pt, elev)
            })
            .collect();
        Self {
            points,
            is_polygon: poly.is_polygon,
        }
    }
}

struct TreeCache {
    tile_zoom: u8,
    tile_x_min: u32,
    tile_x_max: u32,
    tile_y_min: u32,
    tile_y_max: u32,
    data_gen: u64,
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
    let tile_zoom = road_tile_zoom(render_zoom);
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);

    let (x0, y0) = osm_ingest::lat_lon_to_tile(bounds.max_lat, bounds.min_lon, tile_zoom);
    let (x1, y1) = osm_ingest::lat_lon_to_tile(bounds.min_lat, bounds.max_lon, tile_zoom);
    let (txmin, txmax) = (x0.min(x1), x0.max(x1));
    let (tymin, tymax) = (y0.min(y1), y0.max(y1));
    const MARGIN: u32 = 1;
    let current_gen = osm_ingest::road_data_generation();

    {
        let mut store = match tree_cache().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let stale = store.cache.as_ref().map_or(true, |c| {
            c.tile_zoom != tile_zoom
                || c.data_gen != current_gen
                || c.tile_x_min > txmin
                || c.tile_x_max < txmax
                || c.tile_y_min > tymin
                || c.tile_y_max < tymax
        });

        if stale && !store.building {
            let (lxmin, lxmax) = (txmin.saturating_sub(MARGIN), txmax + MARGIN);
            let (lymin, lymax) = (tymin.saturating_sub(MARGIN), tymax + MARGIN);
            store.building = true;
            drop(store);

            let root_buf = root.to_path_buf();
            std::thread::spawn(move || {
                let polylines = load_features_from_cells(&root_buf, "tree", bounds);
                let features = polylines
                    .iter()
                    .map(|p| ElevatedTree::from_polyline(p, Some(&root_buf)))
                    .collect();

                if let Ok(mut store) = tree_cache().lock() {
                    store.cache = Some(TreeCache {
                        tile_zoom,
                        tile_x_min: lxmin,
                        tile_x_max: lxmax,
                        tile_y_min: lymin,
                        tile_y_max: lymax,
                        data_gen: current_gen,
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

    let fill_color = theme::tree_color();
    let thin_stroke = egui::epaint::PathStroke::new(0.6, egui::Color32::from_rgb(20, 80, 30));

    for feat in &cache.features {
        let mut pts: Vec<egui::Pos2> = Vec::with_capacity(feat.points.len());
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

        if pts.len() < 3 {
            continue;
        }

        if feat.is_polygon {
            painter.add(egui::Shape::Path(egui::epaint::PathShape {
                points: pts,
                closed: true,
                fill: fill_color,
                stroke: thin_stroke.clone(),
            }));
        } else if pts.len() >= 2 {
            painter.add(egui::Shape::line(pts, egui::Stroke::new(0.8, fill_color)));
        }
    }
}
