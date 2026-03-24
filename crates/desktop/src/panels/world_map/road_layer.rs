use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest::{self, RoadLayerKind, RoadPolyline};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::local_terrain_scene::{
    local_geo_bounds, project_local, road_tile_zoom, visual_half_extent_for_zoom, LocalLayout,
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
    /// Build an elevated road, sampling SRTM elevation at every `elev_step`-th
    /// vertex and linearly interpolating the rest.  Use `elev_step = 1` for
    /// major roads (full fidelity) and a larger value for minor roads to cap
    /// the number of expensive per-point SRTM lookups.
    fn from_polyline(
        poly: &osm_ingest::RoadPolyline,
        selected_root: Option<&Path>,
        elev_step: usize,
    ) -> Self {
        let pts = &poly.points;
        let n = pts.len();
        if n == 0 {
            return Self { points: Vec::new() };
        }
        let step = elev_step.max(1);

        // Sample elevation at every `step`-th index (always including last).
        let mut sampled: Vec<(usize, f32)> = (0..n)
            .step_by(step)
            .map(|i| {
                let e = srtm_stream::sample_elevation_m(selected_root, pts[i]).unwrap_or(0.0) + 3.0;
                (i, e)
            })
            .collect();
        if sampled.last().map(|&(i, _)| i) != Some(n - 1) {
            let e = srtm_stream::sample_elevation_m(selected_root, pts[n - 1]).unwrap_or(0.0) + 3.0;
            sampled.push((n - 1, e));
        }

        // Linearly interpolate elevations for skipped vertices.
        let mut elevations = vec![0.0f32; n];
        for w in sampled.windows(2) {
            let (i0, e0) = w[0];
            let (i1, e1) = w[1];
            for i in i0..=i1 {
                let t = if i1 > i0 { (i - i0) as f32 / (i1 - i0) as f32 } else { 0.0 };
                elevations[i] = e0 + (e1 - e0) * t;
            }
        }

        let points = pts.iter().zip(elevations).map(|(&pt, e)| (pt, e)).collect();
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
    /// Raw geometry from SQLite — built in a background thread (no SRTM I/O).
    major_polys: Vec<RoadPolyline>,
    minor_polys: Vec<RoadPolyline>,
    /// Elevation-enriched roads, populated lazily on first render so that SRTM
    /// tiles are guaranteed to be in the hot tile-LRU when we sample them.
    major_elevated: Option<Vec<ElevatedRoad>>,
    minor_elevated: Option<Vec<ElevatedRoad>>,
}

struct RoadCacheStore {
    cache: Option<RoadCache>,
    /// True while a background thread is building new geometry.
    building: bool,
}

fn road_cache() -> &'static Mutex<RoadCacheStore> {
    static CACHE: OnceLock<Mutex<RoadCacheStore>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(RoadCacheStore { cache: None, building: false }))
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
        if let Ok(mut g) = road_cache().lock() { g.cache = None; }
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

            // No SRTM I/O in the background thread — geometry only.
            // Elevation is sampled lazily on the first render call so that the
            // SRTM tile LRU is guaranteed warm (contours have already loaded tiles).
            let root_buf = selected_root.map(|p| p.to_path_buf());
            std::thread::spawn(move || {
                let root_ref = root_buf.as_deref();
                let major_polys = if show_major_roads {
                    osm_ingest::load_roads_for_bounds(root_ref, bounds, tile_zoom, RoadLayerKind::Major)
                } else { Vec::new() };
                let minor_polys = if show_minor_roads {
                    osm_ingest::load_roads_for_bounds(root_ref, bounds, tile_zoom, RoadLayerKind::Minor)
                } else { Vec::new() };

                if let Ok(mut store) = road_cache().lock() {
                    store.cache = Some(RoadCache {
                        tile_zoom,
                        tile_x_min: lxmin, tile_x_max: lxmax,
                        tile_y_min: lymin, tile_y_max: lymax,
                        road_gen: current_gen,
                        had_major: show_major_roads,
                        had_minor: show_minor_roads,
                        major_polys,
                        minor_polys,
                        major_elevated: None,
                        minor_elevated: None,
                    });
                    store.building = false;
                }
                crate::app::request_repaint();
            });
        }
        // `store` dropped here (or already explicitly dropped above)
    }

    // ── Render from whatever cache is currently ready ───────────────────
    // Lazily build elevation-enriched roads on first render after a cache
    // update.  By now the SRTM tile LRU is warm (contour rendering already
    // loaded the tiles), so every sample_elevation_m call hits the in-memory
    // tile cache instead of disk.
    let mut store = match road_cache().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(cache) = &mut store.cache else { return };

    if show_major_roads && cache.major_elevated.is_none() {
        cache.major_elevated = Some(
            cache.major_polys.iter()
                .map(|p| ElevatedRoad::from_polyline(p, selected_root, 1))
                .collect(),
        );
    }
    if show_minor_roads && cache.minor_elevated.is_none() {
        cache.minor_elevated = Some(
            cache.minor_polys.iter()
                .map(|p| ElevatedRoad::from_polyline(p, selected_root, 5))
                .collect(),
        );
    }

    if show_minor_roads {
        if let Some(minor) = &cache.minor_elevated {
            draw_road_layer(painter, layout, view, viewport_center,
                extent_x_km, extent_y_km, minor,
                egui::Stroke::new(0.8, egui::Color32::from_rgb(116, 132, 142)));
        }
    }
    if show_major_roads {
        if let Some(major) = &cache.major_elevated {
            draw_road_layer(painter, layout, view, viewport_center,
                extent_x_km, extent_y_km, major,
                egui::Stroke::new(1.35, egui::Color32::from_rgb(255, 210, 92)));
        }
    }
}

pub(super) fn draw_road_layer(
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
        let points: Vec<_> = road
            .points
            .iter()
            .filter_map(|&(pt, elev)| {
                // Elevation is already pre-sampled — this is pure projection math.
                project_local(layout, view, viewport_center, pt, elev,
                              extent_x_km, extent_y_km)
                    .map(|p| p.pos)
            })
            .collect();

        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}
