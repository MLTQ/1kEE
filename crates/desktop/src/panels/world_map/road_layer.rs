use crate::model::{GeoPoint, GlobeViewState};
use crate::osm_ingest::{self, RoadLayerKind};
use crate::theme;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::local_terrain_scene::{
    LocalLayout, local_geo_bounds, project_local, visual_half_extent_for_zoom,
};
use super::srtm_stream;

const MAX_SOURCE_POINTS_PER_ROAD: usize = 192;
const MAX_MAJOR_RENDER_POINTS_TOTAL: usize = 400_000;
const MAX_MINOR_RENDER_POINTS_TOTAL: usize = 800_000;

// How much extra area to pre-fetch beyond the visible viewport in each
// direction, expressed as a fraction of the current view half-extent.
// 0.75 means "load 75 % extra on every side", giving a comfortable pan
// buffer without flooding memory on large zoom-out views.
const GEO_MARGIN_FACTOR: f32 = 0.75;

// ── Road geo-bounds cache ───────────────────────────────────────────────────
// Roads are loaded once for a geo bounding box that is slightly larger than
// the visible viewport.  The cache stays valid as long as the viewport is
// fully contained within that box — zoom changes that shrink the view never
// invalidate it, and zoom-out/pan only invalidates when the viewport actually
// escapes the loaded coverage area.

/// A road polyline with elevation pre-sampled for every vertex.
/// Elevation is computed once at cache-load time so `draw_road_layer`
/// only has to do fast projection math on each frame.
struct ElevatedRoad {
    points: Vec<(GeoPoint, f32)>, // (position, elevation_m above ground)
}

impl ElevatedRoad {
    /// Build an elevated road with a terrain sample for every vertex.
    fn from_polyline(poly: &osm_ingest::RoadPolyline, selected_root: Option<&Path>) -> Self {
        let simplified = simplify_source_points(&poly.points, MAX_SOURCE_POINTS_PER_ROAD);
        if simplified.is_empty() {
            return Self { points: Vec::new() };
        }
        let points = simplified
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

/// Clear the road cache so the next draw reloads from disk.
/// Call this whenever the road layer checkboxes change.
pub fn invalidate_road_cache() {
    if let Ok(mut g) = road_cache().lock() {
        g.cache = None;
        // Leave `building` alone — any in-flight thread will finish and
        // write a result; the stale check will then trigger a fresh build.
    }
}

/// The bounding box of the background road-cache build in progress, if any.
pub fn road_cache_building_bounds() -> Option<osm_ingest::GeoBounds> {
    road_cache().lock().ok().and_then(|g| g.building)
}

struct RoadCache {
    road_gen: u64,
    /// The selected_root active when this cache was built.
    /// A root change (different event) immediately invalidates the cache.
    last_root: Option<std::path::PathBuf>,
    /// Geo coverage this cache was built for (viewport + margin).
    /// Cache is valid as long as the current viewport is fully inside this box.
    /// This is zoom-level independent — zooming in never evicts the cache.
    covered_min_lat: f32,
    covered_max_lat: f32,
    covered_min_lon: f32,
    covered_max_lon: f32,
    major_elevated: Vec<ElevatedRoad>,
    minor_elevated: Vec<ElevatedRoad>,
}

struct RoadCacheStore {
    cache: Option<RoadCache>,
    building: Option<osm_ingest::GeoBounds>,
}

fn road_cache() -> &'static Mutex<RoadCacheStore> {
    static CACHE: OnceLock<Mutex<RoadCacheStore>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(RoadCacheStore {
            cache: None,
            building: None,
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
    // Dynamically calculate the SQLite `road_tiles` zoom level to query based on render depth.
    // If we're fully zoomed out, this drops to 4, preventing 1,000,000-tile queries!
    let tile_zoom = super::local_terrain_scene::road_tile_zoom(render_zoom);

    if !show_major_roads && !show_minor_roads {
        if let Ok(mut g) = road_cache().lock() {
            g.cache = None;
        }
        return;
    }

    let bounds = local_geo_bounds(viewport_center, view.local_zoom);
    let half_extent_deg = visual_half_extent_for_zoom(view.local_zoom);
    let km_per_deg_lat = 111.32f32;
    let km_per_deg_lon = km_per_deg_lat * viewport_center.lat.to_radians().cos().abs().max(0.2);
    let extent_x_km = (half_extent_deg * km_per_deg_lon).max(1.0);
    let extent_y_km = (half_extent_deg * km_per_deg_lat).max(1.0);
    let current_gen = osm_ingest::road_data_generation();

    // ── Stale check + background build launch ─────────────────────────────
    {
        let mut store = match road_cache().lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        // Stale when: data generation changed, toggles changed, root changed,
        // or — crucially — the viewport has panned/zoomed OUT of the loaded
        // geo coverage.  Zooming IN never triggers a rebuild.
        let stale = store.cache.as_ref().map_or(true, |c| {
            c.road_gen != current_gen
                || c.last_root.as_deref() != selected_root
                || bounds.min_lat < c.covered_min_lat
                || bounds.max_lat > c.covered_max_lat
                || bounds.min_lon < c.covered_min_lon
                || bounds.max_lon > c.covered_max_lon
        });

        if stale && store.building.is_none() {
            // Build a load bbox that extends GEO_MARGIN_FACTOR beyond the
            // current viewport in each direction.
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
            drop(store); // release lock before spawning

            let root_buf = selected_root.map(|p| p.to_path_buf());
            std::thread::spawn(move || {
                let root_ref = root_buf.as_deref();
                // Load both classes whenever any road layer is enabled so the
                // cache survives checkbox toggles and only drawing changes.
                // tile_zoom is scaled based on render depth to avoid global grid locks.
                let major_elevated = osm_ingest::load_roads_for_bounds(
                    root_ref,
                    load_bounds,
                    tile_zoom,
                    RoadLayerKind::Major,
                )
                .into_iter()
                .map(|poly| ElevatedRoad::from_polyline(&poly, root_ref))
                .collect::<Vec<_>>();
                let minor_elevated = osm_ingest::load_roads_for_bounds(
                    root_ref,
                    load_bounds,
                    tile_zoom,
                    RoadLayerKind::Minor,
                )
                .into_iter()
                .map(|poly| ElevatedRoad::from_polyline(&poly, root_ref))
                .collect::<Vec<_>>();

                let mut major_elevated = major_elevated;
                let mut minor_elevated = minor_elevated;
                sort_roads_for_budget(&mut major_elevated);
                sort_roads_for_budget(&mut minor_elevated);

                if let Ok(mut store) = road_cache().lock() {
                    store.cache = Some(RoadCache {
                        road_gen: current_gen,
                        last_root: root_buf.clone(),
                        covered_min_lat,
                        covered_max_lat,
                        covered_min_lon,
                        covered_max_lon,
                        major_elevated,
                        minor_elevated,
                    });
                    store.building = None;
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

    if show_major_roads {
        let mut remaining_points = MAX_MAJOR_RENDER_POINTS_TOTAL;
        draw_road_layer(
            painter,
            layout,
            view,
            viewport_center,
            extent_x_km,
            extent_y_km,
            &cache.major_elevated,
            egui::Stroke::new(1.35, theme::road_major_color()),
            &mut remaining_points,
        );
    }
    if show_minor_roads {
        let mut remaining_points = MAX_MINOR_RENDER_POINTS_TOTAL;
        draw_road_layer(
            painter,
            layout,
            view,
            viewport_center,
            extent_x_km,
            extent_y_km,
            &cache.minor_elevated,
            egui::Stroke::new(0.8, theme::road_minor_color()),
            &mut remaining_points,
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
    remaining_points: &mut usize,
) {
    for road in roads {
        if *remaining_points < 2 {
            break;
        }

        let mut points = Vec::with_capacity(road.points.len().min(*remaining_points));
        for &(pt, elev) in &road.points {
            if *remaining_points == 0 {
                break;
            }
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
            *remaining_points = remaining_points.saturating_sub(1);
        }

        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}

fn simplify_source_points(points: &[GeoPoint], max_points: usize) -> Vec<GeoPoint> {
    if points.len() <= max_points {
        return points.to_vec();
    }

    let mut simplified = Vec::with_capacity(max_points);
    simplified.push(points[0]);

    let stride = ((points.len() - 1) as f32 / (max_points - 1) as f32).ceil() as usize;
    let mut idx = stride;
    while idx + 1 < points.len() {
        simplified.push(points[idx]);
        idx += stride;
    }

    let last = *points.last().unwrap();
    if simplified.last().copied() != Some(last) {
        simplified.push(last);
    }

    simplified
}

fn sort_roads_for_budget(roads: &mut [ElevatedRoad]) {
    roads.sort_by(|a, b| {
        b.points
            .len()
            .cmp(&a.points.len())
            .then_with(|| compare_road_start(a, b))
    });
}

fn compare_road_start(a: &ElevatedRoad, b: &ElevatedRoad) -> std::cmp::Ordering {
    let a0 = a
        .points
        .first()
        .map(|(pt, _)| (pt.lat.to_bits(), pt.lon.to_bits()));
    let b0 = b
        .points
        .first()
        .map(|(pt, _)| (pt.lat.to_bits(), pt.lon.to_bits()));
    a0.cmp(&b0)
}
