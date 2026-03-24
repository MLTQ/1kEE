use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest::{self, RoadLayerKind};
use crate::theme;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::local_terrain_scene::{
    LocalLayout, local_geo_bounds, project_local, road_tile_zoom, visual_half_extent_for_zoom,
};
use super::srtm_stream;

// ── Road tile cache ────────────────────────────────────────────────────────
// Road geometry is fetched from SQLite and cached until the tile coverage
// actually changes.  Opening a DB connection + running a query on every
// frame was the source of the 2-5 FPS regression.

/// A road polyline with elevation pre-sampled for every vertex.
/// Elevation is computed once at cache-load time so `draw_road_layer`
/// only has to do fast projection math on each frame.
struct ElevatedRoad {
    points: Vec<(GeoPoint, f32)>, // (position, elevation_m above ground)
}

impl ElevatedRoad {
    /// Build an elevated road with a terrain sample for every vertex.
    fn from_polyline(poly: &osm_ingest::RoadPolyline, selected_root: Option<&Path>) -> Self {
        let pts = &poly.points;
        if pts.is_empty() {
            return Self { points: Vec::new() };
        }

        let points = pts
            .iter()
            .copied()
            .map(|pt| {
                let elevation =
                    srtm_stream::sample_elevation_m(selected_root, pt).unwrap_or(0.0) + 3.0;
                (pt, elevation)
            })
            .collect();
        Self { points }
    }
}

/// Clear the road tile cache so the next draw reloads from SQLite.
/// Call this whenever the road layer checkboxes change.
pub fn invalidate_road_cache() {
    if let Ok(mut g) = road_cache().lock() {
        g.cache = None;
        // Leave `building` alone — any in-flight thread will finish and
        // write a result; the stale check will then trigger a fresh build.
    }
}

/// True while a background road-cache build is in progress.
pub fn road_cache_building() -> bool {
    road_cache().lock().map(|g| g.building).unwrap_or(false)
}

struct RoadCache {
    tile_zoom: u8,
    tile_x_min: u32,
    tile_x_max: u32,
    tile_y_min: u32,
    tile_y_max: u32,
    road_gen: u64,
    had_major: bool,
    had_minor: bool,
    /// Elevation-enriched roads, built off the render thread so enabling
    /// layers or panning into uncached tiles does not hitch the UI.
    major_elevated: Vec<ElevatedRoad>,
    minor_elevated: Vec<ElevatedRoad>,
}

struct RoadCacheStore {
    cache: Option<RoadCache>,
    /// True while a background thread is building new geometry.
    building: bool,
}

fn road_cache() -> &'static Mutex<RoadCacheStore> {
    static CACHE: OnceLock<Mutex<RoadCacheStore>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(RoadCacheStore {
            cache: None,
            building: false,
        })
    })
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
    if !show_major_roads && !show_minor_roads {
        if let Ok(mut g) = road_cache().lock() {
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

    let (x0, y0) = osm_ingest::lat_lon_to_tile(bounds.max_lat, bounds.min_lon, tile_zoom);
    let (x1, y1) = osm_ingest::lat_lon_to_tile(bounds.min_lat, bounds.max_lon, tile_zoom);
    let (txmin, txmax) = (x0.min(x1), x0.max(x1));
    let (tymin, tymax) = (y0.min(y1), y0.max(y1));
    const MARGIN: u32 = 1;
    let current_gen = osm_ingest::road_data_generation();

    // ── Stale check + background build launch ─────────────────────────────
    {
        let mut store = match road_cache().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let stale = store.cache.as_ref().map_or(true, |c| {
            c.tile_zoom != tile_zoom
                || c.road_gen != current_gen
                || c.had_major != show_major_roads
                || c.had_minor != show_minor_roads
                || c.tile_x_min > txmin
                || c.tile_x_max < txmax
                || c.tile_y_min > tymin
                || c.tile_y_max < tymax
        });

        if stale && !store.building {
            let (lxmin, lxmax) = (txmin.saturating_sub(MARGIN), txmax + MARGIN);
            let (lymin, lymax) = (tymin.saturating_sub(MARGIN), tymax + MARGIN);
            store.building = true;
            drop(store); // release lock before spawning

            let root_buf = selected_root.map(|p| p.to_path_buf());
            std::thread::spawn(move || {
                let root_ref = root_buf.as_deref();
                let major_elevated = if show_major_roads {
                    osm_ingest::load_roads_for_bounds(
                        root_ref,
                        bounds,
                        tile_zoom,
                        RoadLayerKind::Major,
                    )
                    .into_iter()
                    .map(|poly| ElevatedRoad::from_polyline(&poly, root_ref))
                    .collect()
                } else {
                    Vec::new()
                };
                let minor_elevated = if show_minor_roads {
                    osm_ingest::load_roads_for_bounds(
                        root_ref,
                        bounds,
                        tile_zoom,
                        RoadLayerKind::Minor,
                    )
                    .into_iter()
                    .map(|poly| ElevatedRoad::from_polyline(&poly, root_ref))
                    .collect()
                } else {
                    Vec::new()
                };

                if let Ok(mut store) = road_cache().lock() {
                    store.cache = Some(RoadCache {
                        tile_zoom,
                        tile_x_min: lxmin,
                        tile_x_max: lxmax,
                        tile_y_min: lymin,
                        tile_y_max: lymax,
                        road_gen: current_gen,
                        had_major: show_major_roads,
                        had_minor: show_minor_roads,
                        major_elevated,
                        minor_elevated,
                    });
                    store.building = false;
                }
                crate::app::request_repaint();
            });
        }
        // `store` dropped here (or already explicitly dropped above)
    }

    let mut store = match road_cache().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(cache) = &mut store.cache else {
        return;
    };

    if show_minor_roads {
        draw_road_layer(
            painter,
            layout,
            view,
            viewport_center,
            extent_x_km,
            extent_y_km,
            &cache.minor_elevated,
            egui::Stroke::new(0.8, theme::road_minor_color()),
        );
    }
    if show_major_roads {
        draw_road_layer(
            painter,
            layout,
            view,
            viewport_center,
            extent_x_km,
            extent_y_km,
            &cache.major_elevated,
            egui::Stroke::new(1.35, theme::road_major_color()),
        );
    }
}

fn draw_road_layer(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    viewport_center: GeoPoint,
    extent_x_km: f32,
    extent_y_km: f32,
    roads: &[ElevatedRoad],
    stroke: egui::Stroke,
) {
    for road in roads {
        let mut points = Vec::with_capacity(road.points.len());
        for &(pt, elev) in &road.points {
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
            points.push(pos);
        }

        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}
