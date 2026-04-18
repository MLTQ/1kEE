/// Generic infrastructure layer renderer.
///
/// Handles RAIL, PIPE, AERO, MILT, COMM, INDS, PORT, GOVT, SURV — all share
/// the same load-from-cells + project-and-draw pattern, differing only in the
/// cell-format prefix, stroke color/width, and whether polygons are filled.
///
/// Each layer type gets its own `OnceLock<Mutex<InfraCacheStore>>` so they
/// can load and invalidate independently.
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

// ── Generic cache ─────────────────────────────────────────────────────────────

struct InfraCache {
    data_gen: u64,
    last_root: std::path::PathBuf,
    covered_min_lat: f32,
    covered_max_lat: f32,
    covered_min_lon: f32,
    covered_max_lon: f32,
    features: Vec<LoadedPolyline>,
}

struct InfraCacheStore {
    cache: Option<InfraCache>,
    building: Option<osm_ingest::GeoBounds>,
}

impl InfraCacheStore {
    const fn empty() -> Self {
        Self {
            cache: None,
            building: None,
        }
    }
}

// ── Per-type static caches ────────────────────────────────────────────────────

macro_rules! infra_cache {
    ($name:ident) => {
        fn $name() -> &'static Mutex<InfraCacheStore> {
            static C: OnceLock<Mutex<InfraCacheStore>> = OnceLock::new();
            C.get_or_init(|| Mutex::new(InfraCacheStore::empty()))
        }
    };
}

infra_cache!(rail_cache);
infra_cache!(pipeline_cache);
infra_cache!(aeroway_cache);
infra_cache!(military_cache);
infra_cache!(comm_cache);
infra_cache!(industrial_cache);
infra_cache!(port_cache);
infra_cache!(govt_cache);
infra_cache!(surv_cache);

// ── Public build-bounds queries ───────────────────────────────────────────────

pub fn rail_cache_building_bounds() -> Option<osm_ingest::GeoBounds> {
    rail_cache().lock().ok().and_then(|g| g.building)
}
pub fn pipeline_cache_building_bounds() -> Option<osm_ingest::GeoBounds> {
    pipeline_cache().lock().ok().and_then(|g| g.building)
}
pub fn aeroway_cache_building_bounds() -> Option<osm_ingest::GeoBounds> {
    aeroway_cache().lock().ok().and_then(|g| g.building)
}
pub fn military_cache_building_bounds() -> Option<osm_ingest::GeoBounds> {
    military_cache().lock().ok().and_then(|g| g.building)
}

// ── Core render helper ────────────────────────────────────────────────────────

fn draw_infra(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    show: bool,
    cache_fn: fn() -> &'static Mutex<InfraCacheStore>,
    prefix: &'static str,
    stroke_for_class: impl Fn(&str) -> egui::Stroke,
    fill_for_class: impl Fn(&str) -> Option<egui::Color32>,
) {
    puffin::profile_function!();

    if !show {
        if let Ok(mut g) = cache_fn().lock() {
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

    {
        let mut store = match cache_fn().lock() {
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
                let features = load_features_from_cells(&root_buf, prefix, load_bounds);

                if let Ok(mut store) = cache_fn().lock() {
                    store.cache = Some(InfraCache {
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

    let store = match cache_fn().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(cache) = &store.cache else { return };

    static DRAW_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let should_log = !DRAW_LOGGED.load(std::sync::atomic::Ordering::Relaxed);

    for feat in &cache.features {
        let raw_pts = feat.points.len();
        let pts: Vec<egui::Pos2> = feat
            .points
            .iter()
            .filter_map(|&pt| {
                project_local(layout, view, viewport_center, pt, 0.0, extent_x_km, extent_y_km)
                    .map(|p| p.pos)
            })
            .collect();

        if should_log {
            eprintln!(
                "[infra draw] prefix={prefix} class={} raw_pts={raw_pts} projected={} poly={}",
                feat.class, pts.len(), feat.is_polygon
            );
        }

        if pts.len() < 2 {
            continue;
        }

        let stroke = stroke_for_class(&feat.class);

        if feat.is_polygon {
            if let Some(fill) = fill_for_class(&feat.class) {
                painter.add(egui::Shape::convex_polygon(pts.clone(), fill, stroke));
                continue;
            }
        }
        painter.add(egui::Shape::line(pts, stroke));
    }
    if should_log && !cache.features.is_empty() {
        DRAW_LOGGED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

// ── Public draw functions ─────────────────────────────────────────────────────

pub(super) fn draw_rail(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    show: bool,
) {
    draw_infra(
        painter, layout, view, selected_root, viewport_center, show,
        rail_cache,
        "railway",
        |cls| {
            let (color, width) = match cls {
                "mainline" | "rail" => (theme::rail_color(), 1.6),
                "subway" => (theme::rail_metro_color(), 1.4),
                "tram" | "light_rail" => (theme::rail_tram_color(), 1.1),
                "disused" => (theme::rail_disused_color(), 0.8),
                _ => (theme::rail_color(), 1.0),
            };
            egui::Stroke::new(width, color)
        },
        |_| None,
    );
}

pub(super) fn draw_pipelines(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    show: bool,
) {
    draw_infra(
        painter, layout, view, selected_root, viewport_center, show,
        pipeline_cache,
        "pipeline",
        |cls| {
            let color = match cls {
                "gas" => theme::pipeline_gas_color(),
                "oil" => theme::pipeline_oil_color(),
                "water" | "sewer" => theme::pipeline_water_color(),
                _ => theme::pipeline_other_color(),
            };
            egui::Stroke::new(1.2, color)
        },
        |_| None,
    );
}

pub(super) fn draw_aeroways(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    show: bool,
) {
    draw_infra(
        painter, layout, view, selected_root, viewport_center, show,
        aeroway_cache,
        "aeroway",
        |cls| {
            let width = if cls == "runway" { 2.5 } else { 1.2 };
            egui::Stroke::new(width, theme::runway_color())
        },
        |cls| match cls {
            "intl_airport" | "dom_airport" | "airfield" | "airstrip" | "terminal" => {
                Some(theme::aeroway_color())
            }
            _ => None,
        },
    );
}

pub(super) fn draw_military(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    show: bool,
) {
    draw_infra(
        painter, layout, view, selected_root, viewport_center, show,
        military_cache,
        "military",
        |_| egui::Stroke::new(1.2, theme::military_color()),
        |_| Some(theme::military_color()),
    );
}

pub(super) fn draw_comm(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    show: bool,
) {
    draw_infra(
        painter, layout, view, selected_root, viewport_center, show,
        comm_cache,
        "comm",
        |_| egui::Stroke::new(1.0, theme::comm_color()),
        |_| None,
    );
}

pub(super) fn draw_industrial(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    show: bool,
) {
    draw_infra(
        painter, layout, view, selected_root, viewport_center, show,
        industrial_cache,
        "industrial",
        |_| egui::Stroke::new(1.0, theme::industrial_color()),
        |cls| match cls {
            "industrial" | "mine" => Some(theme::industrial_color()),
            _ => None,
        },
    );
}

pub(super) fn draw_port(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    show: bool,
) {
    draw_infra(
        painter, layout, view, selected_root, viewport_center, show,
        port_cache,
        "port",
        |_| egui::Stroke::new(1.1, theme::port_color()),
        |cls| match cls {
            "harbour" | "marina" | "shipyard" => Some(theme::port_color()),
            _ => None,
        },
    );
}

pub(super) fn draw_government(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    show: bool,
) {
    draw_infra(
        painter, layout, view, selected_root, viewport_center, show,
        govt_cache,
        "government",
        |_| egui::Stroke::new(1.0, theme::government_color()),
        |_| Some(theme::government_color()),
    );
}

pub(super) fn draw_surveillance(
    painter: &egui::Painter,
    layout: &LocalLayout,
    view: &GlobeViewState,
    selected_root: Option<&Path>,
    viewport_center: GeoPoint,
    _render_zoom: f32,
    show: bool,
) {
    draw_infra(
        painter, layout, view, selected_root, viewport_center, show,
        surv_cache,
        "surveillance",
        |_| egui::Stroke::new(0.9, theme::surveillance_color()),
        |_| None,
    );
}
