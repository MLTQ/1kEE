use crate::args::RoadsBboxCommand;
use crate::geojson::{ensure_cache_dir, merge_write_cells};
use crate::node_store::NodeStore;
use crate::util::{
    GeoBounds, GeoPoint, RoadPolyline, bounds_intersect, canonical_road_class, expand_bounds,
    focus_cells_for_bounds, point_in_bounds, polyline_bounds,
};
use osmpbf::{Element, ElementReader};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const ROAD_FLUSH_THRESHOLD: usize = 10_000;
const NODE_INSERT_BATCH: usize = 50_000;

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
    let node_store_path = candidate_node_store_path(&command.cache_dir, bounds);
    progress(RoadBuildProgress {
        stage: "Scanning Nodes".to_owned(),
        fraction: 0.02,
        message: format!(
            "Scanning nodes in bbox [{:.4},{:.4}] x [{:.4},{:.4}]",
            bounds.min_lat, bounds.max_lat, bounds.min_lon, bounds.max_lon
        ),
    });

    let candidate_nodes = load_or_collect_candidate_nodes(
        &command.planet_path,
        expanded,
        &node_store_path,
        progress,
    )?;
    progress(RoadBuildProgress {
        stage: "Scanning Ways".to_owned(),
        fraction: 0.40,
        message: format!("Prepared {} candidate nodes", candidate_nodes.count()?),
    });

    let build_stats = collect_roads_by_cell(
        &command.planet_path,
        &command.cache_dir,
        bounds,
        &candidate_nodes,
        progress,
    )?;
    progress(RoadBuildProgress {
        stage: "Writing Cache".to_owned(),
        fraction: 0.82,
        message: format!(
            "Writing {} road polylines across {} populated cells",
            build_stats.road_count, build_stats.cell_count
        ),
    });
    let summary = format!(
        "Built {} road polylines across {} populated cells; wrote {} cache files into {}",
        build_stats.road_count,
        build_stats.cell_count,
        build_stats.written_cells,
        command.cache_dir.display()
    );
    progress(RoadBuildProgress {
        stage: "Completed".to_owned(),
        fraction: 1.0,
        message: summary.clone(),
    });
    Ok(summary)
}

fn load_or_collect_candidate_nodes(
    planet_path: &Path,
    bounds: GeoBounds,
    node_store_path: &Path,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<NodeStore, String> {
    let mut node_store = NodeStore::open(node_store_path)?;
    if node_store_path.exists() && node_store.is_complete()? {
        let node_count = node_store.count()?;
        progress(RoadBuildProgress {
            stage: "Loaded Node Cache".to_owned(),
            fraction: 0.32,
            message: format!(
                "Loaded {} candidate nodes from {}",
                node_count,
                node_store_path.display()
            ),
        });
        return Ok(node_store);
    }

    node_store.reset()?;
    collect_candidate_nodes(planet_path, bounds, &mut node_store, progress)?;
    node_store.mark_complete()?;
    let node_count = node_store.count()?;
    progress(RoadBuildProgress {
        stage: "Saved Node Cache".to_owned(),
        fraction: 0.35,
        message: format!(
            "Saved {} candidate nodes to {}",
            node_count,
            node_store_path.display()
        ),
    });
    Ok(node_store)
}

fn collect_candidate_nodes(
    planet_path: &Path,
    bounds: GeoBounds,
    node_store: &mut NodeStore,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<(), String> {
    let reader = ElementReader::from_path(planet_path).map_err(|error| {
        format!(
            "Failed to open planet source {}: {error}",
            planet_path.display()
        )
    })?;
    let mut scanned = 0usize;
    let mut kept = 0usize;
    let mut batch = Vec::with_capacity(NODE_INSERT_BATCH);
    reader
        .for_each(|element| {
            match element {
                Element::Node(node) => {
                    let point = GeoPoint {
                        lat: node.lat() as f32,
                        lon: node.lon() as f32,
                    };
                    if point_in_bounds(point, bounds) {
                        batch.push((node.id(), point));
                        kept += 1;
                    }
                }
                Element::DenseNode(node) => {
                    let point = GeoPoint {
                        lat: node.lat() as f32,
                        lon: node.lon() as f32,
                    };
                    if point_in_bounds(point, bounds) {
                        batch.push((node.id(), point));
                        kept += 1;
                    }
                }
                Element::Way(_) | Element::Relation(_) => {}
            }
            if batch.len() >= NODE_INSERT_BATCH {
                let _ = node_store.insert_batch(&batch);
                batch.clear();
            }
            scanned += 1;
            if scanned % 2_000_000 == 0 {
                progress(RoadBuildProgress {
                    stage: "Scanning Nodes".to_owned(),
                    fraction: 0.30,
                    message: format!(
                        "Scanned {} elements; kept {} candidate nodes",
                        scanned, kept
                    ),
                });
            }
        })
        .map_err(|error| error.to_string())?;
    node_store.insert_batch(&batch)?;
    Ok(())
}

struct RoadBuildStats {
    road_count: usize,
    cell_count: usize,
    written_cells: usize,
}

fn collect_roads_by_cell(
    planet_path: &Path,
    cache_dir: &Path,
    bounds: GeoBounds,
    candidate_nodes: &NodeStore,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<RoadBuildStats, String> {
    let reader = ElementReader::from_path(planet_path).map_err(|error| {
        format!(
            "Failed to reopen planet source {}: {error}",
            planet_path.display()
        )
    })?;
    let mut seen_way_ids = HashSet::new();
    let mut roads_by_cell: HashMap<(i32, i32), Vec<RoadPolyline>> = HashMap::new();
    let mut scanned_ways = 0usize;
    let mut road_count = 0usize;
    let mut touched_cells = HashSet::new();
    let mut written_cells = 0usize;
    let mut buffered_roads = 0usize;

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

            let refs: Vec<_> = way.refs().collect();
            let points = candidate_nodes.points_for_refs(&refs).unwrap_or_default();
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
                    touched_cells.insert(cell);
                    roads_by_cell.entry(cell).or_default().push(road.clone());
                }
            }

            road_count += 1;
            buffered_roads += 1;
            scanned_ways += 1;
            if buffered_roads >= ROAD_FLUSH_THRESHOLD {
                if let Ok(count) = flush_road_chunk(cache_dir, &mut roads_by_cell) {
                    written_cells += count;
                    buffered_roads = 0;
                    progress(RoadBuildProgress {
                        stage: "Writing Cache".to_owned(),
                        fraction: 0.76,
                        message: format!("Flushed roads to {} cache files so far", written_cells),
                    });
                }
            }
            if scanned_ways % 250_000 == 0 {
                progress(RoadBuildProgress {
                    stage: "Scanning Ways".to_owned(),
                    fraction: 0.75,
                    message: format!(
                        "Scanned {} ways; kept {} roads across {} cells",
                        scanned_ways,
                        road_count,
                        touched_cells.len()
                    ),
                });
            }
        })
        .map_err(|error| error.to_string())?;

    written_cells += flush_road_chunk(cache_dir, &mut roads_by_cell)?;
    Ok(RoadBuildStats {
        road_count,
        cell_count: touched_cells.len(),
        written_cells,
    })
}

fn flush_road_chunk(
    cache_dir: &Path,
    roads_by_cell: &mut HashMap<(i32, i32), Vec<RoadPolyline>>,
) -> Result<usize, String> {
    if roads_by_cell.is_empty() {
        return Ok(0);
    }
    let batch = std::mem::take(roads_by_cell);
    merge_write_cells(cache_dir, &batch)
}

fn candidate_node_store_path(cache_dir: &Path, bounds: GeoBounds) -> PathBuf {
    cache_dir.join(".builder_state").join(format!(
        "candidate_nodes_{:+08.4}_{:+08.4}_{:+09.4}_{:+09.4}.sqlite",
        bounds.min_lat, bounds.max_lat, bounds.min_lon, bounds.max_lon
    ))
}
