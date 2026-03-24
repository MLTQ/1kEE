use crate::args::RoadsBboxCommand;
use crate::geojson::{ensure_cache_dir, merge_write_cells};
use crate::util::{
    GeoBounds, GeoPoint, RoadPolyline, bounds_intersect, canonical_road_class, expand_bounds,
    focus_cells_for_bounds, point_in_bounds, polyline_bounds,
};
use osmpbf::{Element, ElementReader};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

const ROAD_FLUSH_THRESHOLD: usize = 10_000;

#[derive(Serialize, Deserialize)]
struct CandidateNodeRecord {
    id: i64,
    lat: f32,
    lon: f32,
}

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
    let checkpoint_path = candidate_node_checkpoint_path(&command.cache_dir, bounds);
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
        &checkpoint_path,
        progress,
    )?;
    progress(RoadBuildProgress {
        stage: "Scanning Ways".to_owned(),
        fraction: 0.40,
        message: format!("Collected {} candidate nodes", candidate_nodes.len()),
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
    checkpoint_path: &Path,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<HashMap<i64, GeoPoint>, String> {
    if checkpoint_path.exists() {
        let nodes = load_candidate_nodes_checkpoint(checkpoint_path)?;
        progress(RoadBuildProgress {
            stage: "Loaded Node Cache".to_owned(),
            fraction: 0.32,
            message: format!(
                "Loaded {} candidate nodes from {}",
                nodes.len(),
                checkpoint_path.display()
            ),
        });
        return Ok(nodes);
    }

    let nodes = collect_candidate_nodes(planet_path, bounds, progress)?;
    save_candidate_nodes_checkpoint(checkpoint_path, &nodes)?;
    progress(RoadBuildProgress {
        stage: "Saved Node Cache".to_owned(),
        fraction: 0.35,
        message: format!(
            "Saved {} candidate nodes to {}",
            nodes.len(),
            checkpoint_path.display()
        ),
    });
    Ok(nodes)
}

fn collect_candidate_nodes(
    planet_path: &Path,
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

struct RoadBuildStats {
    road_count: usize,
    cell_count: usize,
    written_cells: usize,
}

fn collect_roads_by_cell(
    planet_path: &Path,
    cache_dir: &Path,
    bounds: GeoBounds,
    candidate_nodes: &HashMap<i64, GeoPoint>,
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
                    fraction: 0.45 + 0.30,
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

fn candidate_node_checkpoint_path(cache_dir: &Path, bounds: GeoBounds) -> PathBuf {
    cache_dir.join(".builder_state").join(format!(
        "candidate_nodes_{:+08.4}_{:+08.4}_{:+09.4}_{:+09.4}.jsonl",
        bounds.min_lat, bounds.max_lat, bounds.min_lon, bounds.max_lon
    ))
}

fn save_candidate_nodes_checkpoint(
    checkpoint_path: &Path,
    nodes: &HashMap<i64, GeoPoint>,
) -> Result<(), String> {
    if let Some(parent) = checkpoint_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp_path = checkpoint_path.with_extension("jsonl.tmp");
    let file = File::create(&temp_path).map_err(|error| error.to_string())?;
    let mut writer = BufWriter::new(file);
    for (&id, point) in nodes {
        let record = CandidateNodeRecord {
            id,
            lat: point.lat,
            lon: point.lon,
        };
        serde_json::to_writer(&mut writer, &record).map_err(|error| error.to_string())?;
        writer.write_all(b"\n").map_err(|error| error.to_string())?;
    }
    writer.flush().map_err(|error| error.to_string())?;
    fs::rename(&temp_path, checkpoint_path).map_err(|error| error.to_string())
}

fn load_candidate_nodes_checkpoint(
    checkpoint_path: &Path,
) -> Result<HashMap<i64, GeoPoint>, String> {
    let file = File::open(checkpoint_path).map_err(|error| error.to_string())?;
    let reader = BufReader::new(file);
    let mut nodes = HashMap::new();
    for line in reader.lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let record: CandidateNodeRecord =
            serde_json::from_str(&line).map_err(|error| error.to_string())?;
        nodes.insert(
            record.id,
            GeoPoint {
                lat: record.lat,
                lon: record.lon,
            },
        );
    }
    Ok(nodes)
}
