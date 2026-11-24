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
use std::sync::{Arc, Mutex, OnceLock};

use super::cell_loader::{LoadedPolyline, load_features_from_cells};
use super::tree_geometry as geometry;
#[cfg(test)]
use super::local_terrain_scene::project_local;
use super::local_terrain_scene::{
    LocalLayout, local_geo_bounds, projection::local_projection_params, visual_half_extent_for_zoom,
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
    features: Vec<InfraFeature>,
}

struct InfraFeature {
    way_id: i64,
    class: String,
    name: Option<String>,
    bounds: [f32; 4],
    points: Vec<(GeoPoint, f32)>,
    fill: Option<geometry::Fill>,
}

impl InfraFeature {
    fn prepare(feature: LoadedPolyline, prefix: &str) -> Self {
        let mut points: Vec<_> = feature.points.into_iter().map(|p| (p, 0.)).collect();
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
        let area = if prefix == "aeroway" {
            matches!(feature.class.as_str(), "runway" | "terminal" | "helipad")
        } else {
            feature.is_polygon
        };
        let fill = area.then(|| geometry::tessellate(&points)).flatten();
        Self {
            way_id: feature.way_id,
            class: feature.class,
            name: feature.name,
            bounds: [min_lon, min_lat, max_lon, max_lat],
            points,
            fill,
        }
    }

    #[cfg(test)]
    fn shapes(
        &self, clip: egui::Rect, stroke: egui::Stroke, color: Option<egui::Color32>,
        project: impl Fn(GeoPoint, f32) -> Option<egui::Pos2> + Copy,
    ) -> Vec<egui::Shape> {
        let mut shapes = Vec::new();
        if let (Some(fill), Some(color)) = (&self.fill, color) {
            let mesh = geometry::project_fill(fill, color, clip, project);
            if !mesh.is_empty() { shapes.push(egui::Shape::mesh(mesh)); }
        }
        shapes.extend(geometry::outlines(&self.points, stroke, project));
        shapes
    }
}

fn aeroway_stroke(class: &str) -> egui::Stroke {
    match class {
        "runway" | "airstrip" => egui::Stroke::new(2.5_f32, theme::runway_color()),
        "intl_airport" | "dom_airport" | "airfield" => {
            egui::Stroke::new(0.7_f32, theme::aeroway_color().gamma_multiply(0.6))
        }
        _ => egui::Stroke::new(1.2_f32, theme::runway_color()),
    }
}

fn aeroway_fill(class: &str) -> Option<egui::Color32> {
    matches!(class, "runway" | "terminal" | "helipad").then(theme::aeroway_color)
}

struct InfraCacheStore {
    cache: Option<Arc<InfraCache>>,
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

    let cache = {
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
            let snapshot = store.cache.clone();
            drop(store);

            let root_buf = root.to_path_buf();
            std::thread::spawn(move || {
                let features = load_features_from_cells(&root_buf, prefix, load_bounds)
                    .into_iter()
                    .map(|feature| InfraFeature::prepare(feature, prefix))
                    .collect();
                let next = Arc::new(InfraCache {
                    data_gen: current_gen,
                    last_root: root_buf,
                    covered_min_lat,
                    covered_max_lat,
                    covered_min_lon,
                    covered_max_lon,
                    features,
                });

                let retired = if let Ok(mut store) = cache_fn().lock() {
                    let old = store.cache.replace(next);
                    store.building = None;
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
    let clip = painter.clip_rect();
    let view_bounds = params
        .ground_bounds(clip.expand(8.0))
        .unwrap_or([-180.0, -90.0, 180.0, 90.0]);
    let span = layout.width.max(layout.height) * 8.0;
    let hover_pointer = if prefix == "pipeline" {
        super::infrastructure_hover::pointer()
    } else {
        None
    };

    // Batch all visible area fills into one shared egui::Mesh.
    let mut fill_mesh = egui::Mesh::default();
    let mut pos_scratch = Vec::new();
    let mut outline_shapes = Vec::new();

    for feat in &cache.features {
        if feat.bounds[3] < view_bounds[1]
            || feat.bounds[1] > view_bounds[3]
            || (feat.bounds[2] < view_bounds[0] && feat.bounds[2] + 360.0 < view_bounds[0])
            || (feat.bounds[0] > view_bounds[2] && feat.bounds[0] - 360.0 > view_bounds[2])
        {
            continue;
        }
        let Some((anchor, _)) = feat.points.first() else { continue };
        let shift = ((viewport_center.lon - anchor.lon) / 360.).round() * 360.;
        let project = |mut point: GeoPoint, height| {
            point.lon += shift;
            let (x, y, _) = params.project_logical(point, height);
            (x.is_finite() && y.is_finite() && x.abs() < span && y.abs() < span)
                .then_some(egui::pos2(x, y))
        };
        if let (Some(fill), Some(color)) = (&feat.fill, fill_for_class(&feat.class)) {
            geometry::append_fill(fill, color, clip, project, &mut fill_mesh, &mut pos_scratch);
        }
        let stroke = stroke_for_class(&feat.class);
        let feat_outlines = geometry::outlines(&feat.points, stroke, project);
        if let Some(pointer) = hover_pointer {
            let distance = feat_outlines
                .iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Path(path) => Some(&path.points),
                    _ => None,
                })
                .flat_map(|pts| pts.windows(2))
                .map(|p| super::infrastructure_hover::line_distance(pointer, p[0], p[1]))
                .fold(f32::INFINITY, f32::min);
            if distance <= super::infrastructure_hover::RADIUS {
                super::infrastructure_hover::pipeline(
                    &tile_archive::pipelines::Info {
                        source: "osm".into(),
                        source_id: feat.way_id.to_string(),
                        name: feat.name.clone().unwrap_or_default(),
                        operator: String::new(),
                        owner: String::new(),
                        product: feat.class.clone(),
                        status: "Not provided in the local cache".into(),
                        accuracy: "OpenStreetMap geometry; precision varies".into(),
                        historical: false,
                        planned: false,
                        capacity: None,
                    },
                    distance,
                );
            }
        }
        outline_shapes.extend(feat_outlines);
    }
    if !fill_mesh.is_empty() {
        painter.add(egui::Shape::mesh(fill_mesh));
    }
    if !outline_shapes.is_empty() {
        painter.extend(outline_shapes);
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
    capacity: bool,
) {
    draw_infra(
        painter, layout, view, selected_root, viewport_center, show,
        pipeline_cache,
        "pipeline",
        |cls| {
            let color = if capacity { super::pipeline_capacity::UNKNOWN } else { match cls {
                "gas" => theme::pipeline_gas_color(),
                "oil" => theme::pipeline_oil_color(),
                "water" | "sewer" => theme::pipeline_water_color(),
                _ => theme::pipeline_other_color(),
            }};
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
        aeroway_stroke,
        aeroway_fill,
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

#[cfg(test)]
#[path = "infra_layer_tests.rs"]
mod tests;
