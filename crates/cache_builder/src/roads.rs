use crate::args::RoadsBboxCommand;
use crate::geojson::{ensure_cache_dir, merge_write_cells};
use crate::util::{
    GeoBounds, GeoPoint, RoadPolyline, bounds_intersect, canonical_road_class, expand_bounds,
    focus_cells_for_bounds, point_in_bounds, polyline_bounds,
};
use osmpbf::{Element, ElementReader};
use std::collections::{HashMap, HashSet};

pub fn build_bbox_cache(command: RoadsBboxCommand) -> Result<(), String> {
    build_bbox_cache_with_progress(command, &mut |_| {}).map(|_| ())
}

#[derive(Clone, Debug)]
pub struct RoadBuildProgress {
    pub stage: String,
    pub fraction: f32,
    pub message: String,
}

pub fn build_bbox_cache_with_progress(
    command: RoadsBboxCommand,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<String, String> {
    if !command.planet_path.exists() {
        return Err(format!(
            "Planet source not found at {}",
            command.planet_path.display()
        ));
    }
    ensure_cache_dir(&command.cache_dir)?;

    let bounds = GeoBounds {
        min_lat: command.min_lat,
        max_lat: command.max_lat,
        min_lon: command.min_lon,
        max_lon: command.max_lon,
    };
    let expanded = expand_bounds(bounds, command.margin_degrees);
    progress(RoadBuildProgress {
        stage: "Scanning Nodes".to_owned(),
        fraction: 0.02,
        message: format!(
            "Scanning nodes in bbox [{:.4},{:.4}] x [{:.4},{:.4}]",
            bounds.min_lat, bounds.max_lat, bounds.min_lon, bounds.max_lon
        ),
    });

    let candidate_nodes = collect_candidate_nodes(&command.planet_path, expanded, progress)?;
    progress(RoadBuildProgress {
        stage: "Scanning Ways".to_owned(),
        fraction: 0.40,
        message: format!("Collected {} candidate nodes", candidate_nodes.len()),
    });

    let roads_by_cell =
        collect_roads_by_cell(&command.planet_path, bounds, &candidate_nodes, progress)?;
    let cell_count = roads_by_cell.len();
    let road_count: usize = roads_by_cell.values().map(Vec::len).sum();
    progress(RoadBuildProgress {
        stage: "Writing Cache".to_owned(),
        fraction: 0.82,
        message: format!(
            "Writing {} road polylines across {} populated cells",
            road_count, cell_count
        ),
    });
    let written_cells = merge_write_cells(&command.cache_dir, &roads_by_cell)?;
    let summary = format!(
        "Built {} road polylines across {} populated cells; wrote {} cache files into {}",
        road_count,
        cell_count,
        written_cells,
        command.cache_dir.display()
    );
    progress(RoadBuildProgress {
        stage: "Completed".to_owned(),
        fraction: 1.0,
        message: summary.clone(),
    });
    Ok(summary)
}

fn collect_candidate_nodes(
    planet_path: &std::path::Path,
    bounds: GeoBounds,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<HashMap<i64, GeoPoint>, String> {
    let reader = ElementReader::from_path(planet_path).map_err(|error| {
        format!(
            "Failed to open planet source {}: {error}",
            planet_path.display()
        )
    })?;
    let mut nodes = HashMap::new();
    let mut scanned = 0usize;
    reader
        .for_each(|element| {
            match element {
                Element::Node(node) => {
                    let point = GeoPoint {
                        lat: node.lat() as f32,
                        lon: node.lon() as f32,
                    };
                    if point_in_bounds(point, bounds) {
                        nodes.insert(node.id(), point);
                    }
                }
                Element::DenseNode(node) => {
                    let point = GeoPoint {
                        lat: node.lat() as f32,
                        lon: node.lon() as f32,
                    };
                    if point_in_bounds(point, bounds) {
                        nodes.insert(node.id(), point);
                    }
                }
                Element::Way(_) | Element::Relation(_) => {}
            }
            scanned += 1;
            if scanned % 2_000_000 == 0 {
                progress(RoadBuildProgress {
                    stage: "Scanning Nodes".to_owned(),
                    fraction: 0.05 + 0.25,
                    message: format!(
                        "Scanned {} elements; kept {} candidate nodes",
                        scanned,
                        nodes.len()
                    ),
                });
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(nodes)
}

fn collect_roads_by_cell(
    planet_path: &std::path::Path,
    bounds: GeoBounds,
    candidate_nodes: &HashMap<i64, GeoPoint>,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<HashMap<(i32, i32), Vec<RoadPolyline>>, String> {
    let reader = ElementReader::from_path(planet_path).map_err(|error| {
        format!(
            "Failed to reopen planet source {}: {error}",
            planet_path.display()
        )
    })?;
    let mut seen_way_ids = HashSet::new();
    let mut roads_by_cell: HashMap<(i32, i32), Vec<RoadPolyline>> = HashMap::new();
    let mut scanned_ways = 0usize;

    reader
        .for_each(|element| {
            let Element::Way(way) = element else {
                return;
            };

            let mut highway_class = None;
            let mut road_name = None;
            for (key, value) in way.tags() {
                if key == "highway" {
                    highway_class = canonical_road_class(value);
                } else if key == "name" && road_name.is_none() {
                    road_name = Some(value.to_owned());
                }
            }
            let Some(road_class) = highway_class else {
                return;
            };
            if !seen_way_ids.insert(way.id()) {
                return;
            }

            let points: Vec<_> = way
                .refs()
                .filter_map(|node_id| candidate_nodes.get(&node_id).copied())
                .collect();
            if points.len() < 2 {
                return;
            }

            let road_bounds = polyline_bounds(&points);
            if !bounds_intersect(road_bounds, bounds) {
                return;
            }

            let road = RoadPolyline {
                way_id: way.id(),
                road_class: road_class.to_owned(),
                name: road_name,
                points,
            };

            let mut assigned_cells = HashSet::new();
            for cell in focus_cells_for_bounds(road_bounds) {
                if assigned_cells.insert(cell) {
                    roads_by_cell.entry(cell).or_default().push(road.clone());
                }
            }

            scanned_ways += 1;
            if scanned_ways % 250_000 == 0 {
                let roads: usize = roads_by_cell.values().map(Vec::len).sum();
                progress(RoadBuildProgress {
                    stage: "Scanning Ways".to_owned(),
                    fraction: 0.45 + 0.30,
                    message: format!(
                        "Scanned {} ways; kept {} roads across {} cells",
                        scanned_ways,
                        roads,
                        roads_by_cell.len()
                    ),
                });
            }
        })
        .map_err(|error| error.to_string())?;

    Ok(roads_by_cell)
}
