use crate::args::BboxCommand;
use crate::geojson::{ensure_cache_dir, merge_write_cells, merge_write_feature_cells};
use crate::node_store::NodeStore;
use crate::srtm::SrtmSampler;
use crate::util::{
    GeoBounds, GeoPoint, RoadPolyline, WayFeature, bounds_intersect, canonical_aeroway_class,
    canonical_building_class, canonical_comm_class, canonical_govt_class,
    canonical_industrial_class, canonical_military_class, canonical_pipeline_class,
    canonical_port_class, canonical_power_class, canonical_railway_class, canonical_road_class,
    canonical_surv_class, canonical_tree_class, canonical_waterway_class, expand_bounds,
    focus_cells_iter, parse_voltage_kv, point_in_bounds, polyline_bounds,
};
use osmpbf::{BlobDecode, BlobReader};
use rayon::prelude::*;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const ROAD_FLUSH_THRESHOLD: usize = 100_000;
const NODE_INSERT_BATCH: usize = 50_000;
const PBF_READ_BUFFER_BYTES: usize = 4 * 1024 * 1024;
const BBOX_BATCH_BLOBS: usize = 64;
/// Bound the work replayed by a resumed node scan even over sparse areas where
/// the retained-node batch does not fill for a long time.
const NODE_CHECKPOINT_SCAN_INTERVAL: usize = 1_000_000;
const NODE_PROGRESS_SCAN_INTERVAL: usize = 2_000_000;

// ── Position-tracking reader ──────────────────────────────────────────────────
//
// Wraps a buffered `File` and counts every byte delivered to the BlobReader.
// Because PBF blobs are length-prefixed and BlobReader reads exactly one blob
// per iterator step, `pos` equals the exact logical file offset of the *start
// of the next* blob after each `next()` call — a safe resume point, while the
// 4 MiB `BufReader` eliminates per-header/per-blob `read()` syscalls.

pub(crate) struct PosReader {
    inner: BufReader<File>,
    pos: Arc<AtomicU64>,
}

impl Read for PosReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.pos.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

// `BufReader<File>` is Send; `Arc<AtomicU64>` is Send.
unsafe impl Send for PosReader {}

/// Open `planet_path` seeked to `start_offset` and wrap in a `BlobReader`.
/// Returns the reader plus an `Arc` that is updated as bytes are consumed,
/// so callers can checkpoint the file position without needing the reader back.
pub(crate) fn open_planet_at(
    planet_path: &Path,
    start_offset: u64,
) -> Result<(BlobReader<PosReader>, Arc<AtomicU64>), String> {
    let mut file = File::open(planet_path)
        .map_err(|e| format!("Cannot open {}: {e}", planet_path.display()))?;
    if start_offset > 0 {
        file.seek(SeekFrom::Start(start_offset))
            .map_err(|e| format!("Seek failed: {e}"))?;
    }
    let pos = Arc::new(AtomicU64::new(start_offset));
    let reader = BlobReader::new(PosReader {
        inner: BufReader::with_capacity(PBF_READ_BUFFER_BYTES, file),
        pos: pos.clone(),
    });
    Ok((reader, pos))
}

// ── Public entry points ───────────────────────────────────────────────────────

pub fn build_bbox_cache(command: BboxCommand) -> Result<(), String> {
    build_bbox_cache_with_progress(command, &mut |_| {}).map(|_| ())
}

#[derive(Clone, Debug)]
pub struct RoadBuildProgress {
    pub metrics: Option<crate::build_monitor::Metrics>,
    pub stage: String,
    pub fraction: f32,
    pub message: String,
}

pub fn build_bbox_cache_with_progress(
    command: BboxCommand,
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
        metrics: None,
        stage: "Scanning Nodes".to_owned(),
        fraction: 0.02,
        message: format!(
            "Scanning nodes in bbox [{:.4},{:.4}] x [{:.4},{:.4}]",
            bounds.min_lat, bounds.max_lat, bounds.min_lon, bounds.max_lon
        ),
    });

    let mut candidate_nodes = load_or_collect_candidate_nodes(
        &command.planet_path,
        expanded,
        &node_store_path,
        progress,
    )?;
    progress(RoadBuildProgress {
        metrics: None,
        stage: "Scanning Ways".to_owned(),
        fraction: 0.40,
        message: format!("Prepared {} candidate nodes", candidate_nodes.count()?),
    });

    let mut srtm = command.srtm_root.clone().map(SrtmSampler::new);

    let build_stats = collect_all_features_by_cell(
        &command,
        bounds,
        &mut candidate_nodes,
        srtm.as_mut(),
        progress,
    )?;

    if command.build_admin {
        progress(RoadBuildProgress {
            metrics: None,
            stage: "Scanning Relations".to_owned(),
            fraction: 0.83,
            message: "Scanning admin boundary relations…".to_owned(),
        });
        crate::admin::load_or_build_admin_boundaries(
            &command,
            bounds,
            &mut candidate_nodes,
            progress,
        )?;
    }

    progress(RoadBuildProgress {
        metrics: None,
        stage: "Writing Cache".to_owned(),
        fraction: 0.82,
        message: format!(
            "Writing {} features across {} populated cells",
            build_stats.feature_count, build_stats.cell_count
        ),
    });
    let summary = format!(
        "Built {} features across {} populated cells; wrote {} cache files into {}",
        build_stats.feature_count,
        build_stats.cell_count,
        build_stats.written_cells,
        command.cache_dir.display()
    );
    progress(RoadBuildProgress {
        metrics: None,
        stage: "Completed".to_owned(),
        fraction: 1.0,
        message: summary.clone(),
    });
    Ok(summary)
}

// ── Pass 1: candidate node collection ────────────────────────────────────────

fn load_or_collect_candidate_nodes(
    planet_path: &Path,
    bounds: GeoBounds,
    node_store_path: &Path,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<NodeStore, String> {
    let mut node_store = NodeStore::open(node_store_path)?;

    if node_store.is_complete()? {
        let node_count = node_store.count()?;
        progress(RoadBuildProgress {
            metrics: None,
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

    // Resumable: if we have a saved offset the node scan was interrupted mid-way.
    // Reuse the existing partial data (INSERT OR REPLACE handles duplicates).
    let resume_offset = node_store.get_scan_offset("node_scan")?;
    if resume_offset.is_none() {
        // Fresh start — wipe any stale partial data.
        node_store.reset()?;
    } else {
        progress(RoadBuildProgress {
            metrics: None,
            stage: "Resuming Node Scan".to_owned(),
            fraction: 0.03,
            message: format!(
                "Resuming node scan from file offset {} ({} nodes collected so far)",
                resume_offset.unwrap(),
                node_store.count()?
            ),
        });
    }

    collect_candidate_nodes(
        planet_path,
        bounds,
        &mut node_store,
        resume_offset,
        progress,
    )?;
    node_store.mark_complete_and_clear_scan_offset("node_scan")?;

    let node_count = node_store.count()?;
    progress(RoadBuildProgress {
        metrics: None,
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
    resume_offset: Option<u64>,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<(), String> {
    let start = resume_offset.unwrap_or(0);
    let (mut reader, pos) = open_planet_at(planet_path, start)?;

    let mut scanned = 0usize;
    let mut kept = 0usize;
    let mut batch: Vec<(i64, GeoPoint)> = Vec::with_capacity(NODE_INSERT_BATCH);
    let mut next_checkpoint_at = NODE_CHECKPOINT_SCAN_INTERVAL;
    let mut next_progress_at = NODE_PROGRESS_SCAN_INTERVAL;

    loop {
        let blobs: Vec<_> = reader
            .by_ref()
            .take(BBOX_BATCH_BLOBS)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        if blobs.is_empty() {
            break;
        }
        let partials: Vec<(usize, Vec<(i64, GeoPoint)>)> = blobs
            .par_iter()
            .map(|blob| -> Result<(usize, Vec<(i64, GeoPoint)>), String> {
                let decoded = blob.decode().map_err(|e| e.to_string())?;
                let BlobDecode::OsmData(block) = decoded else {
                    return Ok((0, Vec::new()));
                };
                let mut local_scanned = 0usize;
                let mut local_kept = Vec::new();
                for element in block.elements() {
                    let (node_id, lat, lon) = match element {
                        osmpbf::Element::Node(n) => (n.id(), n.lat() as f32, n.lon() as f32),
                        osmpbf::Element::DenseNode(n) => (n.id(), n.lat() as f32, n.lon() as f32),
                        _ => continue,
                    };
                    let point = GeoPoint { lat, lon };
                    if point_in_bounds(point, bounds) {
                        local_kept.push((node_id, point));
                    }
                    local_scanned += 1;
                }
                Ok((local_scanned, local_kept))
            })
            .collect::<Result<Vec<_>, _>>()?;

        for (local_scanned, local_kept) in partials {
            scanned += local_scanned;
            kept += local_kept.len();
            batch.extend(local_kept);
        }

        // Keep both the candidate-node rows and the offset in one transaction.
        // Checkpointing based on scanned nodes also prevents huge replay gaps
        // through sparse regions that never fill a retained-node batch.
        if batch.len() >= NODE_INSERT_BATCH || scanned >= next_checkpoint_at {
            let checkpoint = pos.load(Ordering::Relaxed);
            node_store.insert_batch_and_save_scan_offset(&batch, "node_scan", checkpoint)?;
            batch.clear();
            while scanned >= next_checkpoint_at {
                next_checkpoint_at += NODE_CHECKPOINT_SCAN_INTERVAL;
            }

            if scanned >= next_progress_at {
                progress(RoadBuildProgress {
                    metrics: None,
                    stage: "Scanning Nodes".to_owned(),
                    fraction: 0.30,
                    message: format!(
                        "Scanned {} elements; kept {} candidate nodes",
                        scanned, kept
                    ),
                });
                while scanned >= next_progress_at {
                    next_progress_at += NODE_PROGRESS_SCAN_INTERVAL;
                }
            }
        }
    }

    // Persist the final tail and EOF offset together. If completion is
    // interrupted below, the next run starts at EOF instead of rewriting it.
    let checkpoint = pos.load(Ordering::Relaxed);
    node_store.insert_batch_and_save_scan_offset(&batch, "node_scan", checkpoint)?;
    Ok(())
}

// ── Pass 2: combined feature collection by cell ───────────────────────────────

struct FeatureBuildStats {
    feature_count: usize,
    cell_count: usize,
    written_cells: usize,
}

fn collect_all_features_by_cell(
    command: &BboxCommand,
    bounds: GeoBounds,
    candidate_nodes: &mut NodeStore,
    mut srtm: Option<&mut SrtmSampler>,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<FeatureBuildStats, String> {
    // Load all candidate nodes into RAM for O(1) lookups during the way scan,
    // avoiding millions of SQLite queries.
    progress(RoadBuildProgress {
        metrics: None,
        stage: "Loading Node Map".to_owned(),
        fraction: 0.40,
        message: "Loading candidate nodes into memory…".to_owned(),
    });
    let node_map = candidate_nodes.load_all_nodes()?;
    progress(RoadBuildProgress {
        metrics: None,
        stage: "Scanning Ways".to_owned(),
        fraction: 0.42,
        message: format!("Loaded {} nodes into memory", node_map.len()),
    });

    // Resume from the last way-scan checkpoint if available.
    let resume_offset = candidate_nodes.get_scan_offset("way_scan")?;
    if let Some(off) = resume_offset {
        progress(RoadBuildProgress {
            metrics: None,
            stage: "Resuming Way Scan".to_owned(),
            fraction: 0.43,
            message: format!("Resuming way scan from file offset {off}"),
        });
    }

    let start = resume_offset.unwrap_or(0);
    let (mut reader, pos) = open_planet_at(&command.planet_path, start)?;

    // Per-feature-type accumulators
    let mut seen_road_ids: HashSet<i64> = HashSet::new();
    let mut seen_waterway_ids: HashSet<i64> = HashSet::new();
    let mut seen_building_ids: HashSet<i64> = HashSet::new();
    let mut seen_tree_ids: HashSet<i64> = HashSet::new();
    let mut seen_power_ids: HashSet<i64> = HashSet::new();
    let mut seen_rail_ids: HashSet<i64> = HashSet::new();
    let mut seen_pipeline_ids: HashSet<i64> = HashSet::new();
    let mut seen_aeroway_ids: HashSet<i64> = HashSet::new();
    let mut seen_military_ids: HashSet<i64> = HashSet::new();
    let mut seen_comm_ids: HashSet<i64> = HashSet::new();
    let mut seen_industrial_ids: HashSet<i64> = HashSet::new();
    let mut seen_port_ids: HashSet<i64> = HashSet::new();
    let mut seen_govt_ids: HashSet<i64> = HashSet::new();
    let mut seen_surv_ids: HashSet<i64> = HashSet::new();

    let mut roads_by_cell: HashMap<(i32, i32), Vec<RoadPolyline>> = HashMap::new();
    let mut waterways_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut buildings_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut trees_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut power_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut rail_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut pipeline_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut aeroway_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut military_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut comm_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut industrial_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut port_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut govt_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();
    let mut surv_by_cell: HashMap<(i32, i32), Vec<WayFeature>> = HashMap::new();

    struct MatchedBboxWay {
        way_id: i64,
        way_bounds: GeoBounds,
        points: Vec<GeoPoint>,
        name: Option<String>,
        road_class: Option<&'static str>,
        waterway_class: Option<&'static str>,
        building_class: Option<&'static str>,
        tree_class: Option<&'static str>,
        military_class: Option<&'static str>,
        industrial_class: Option<&'static str>,
        port_class: Option<&'static str>,
        govt_class: Option<&'static str>,
        surv_class: Option<&'static str>,
        power_class: Option<&'static str>,
        raw_railway: Option<&'static str>,
        pipeline_class: Option<&'static str>,
        aeroway_class: Option<&'static str>,
        comm_class: Option<&'static str>,
    }

    let mut scanned_ways = 0usize;
    let mut next_progress_ways = 250_000usize;
    let mut feature_count = 0usize;
    let mut touched_cells: HashSet<(i32, i32)> = HashSet::new();
    let mut written_cells = 0usize;
    let mut buffered = 0usize;

    loop {
        let blobs: Vec<_> = reader
            .by_ref()
            .take(BBOX_BATCH_BLOBS)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        if blobs.is_empty() {
            break;
        }

        let partials: Vec<Vec<MatchedBboxWay>> = blobs
            .par_iter()
            .map(|blob| -> Result<Vec<MatchedBboxWay>, String> {
                let decoded = blob.decode().map_err(|e| e.to_string())?;
                let BlobDecode::OsmData(block) = decoded else {
                    return Ok(Vec::new());
                };
                let mut matched = Vec::new();
                let mut points_buf: Vec<GeoPoint> = Vec::new();

                for way in block.groups().flat_map(|g| g.ways()) {
                    let mut road_class: Option<&'static str> = None;
                    let mut waterway_class: Option<&'static str> = None;
                    let mut building_class: Option<&'static str> = None;
                    let mut tree_class: Option<&'static str> = None;
                    let mut military_class: Option<&'static str> = None;
                    let mut industrial_class: Option<&'static str> = None;
                    let mut port_class: Option<&'static str> = None;
                    let mut govt_class: Option<&'static str> = None;
                    let mut surv_class: Option<&'static str> = None;
                    let mut raw_name: Option<&str> = None;

                    let mut raw_power: Option<&str> = None;
                    let mut raw_voltage: Option<&str> = None;
                    let mut raw_railway: Option<&'static str> = None;
                    let mut raw_pipeline = false;
                    let mut raw_substance: Option<&str> = None;
                    let mut raw_aeroway: Option<&str> = None;
                    let mut raw_aerodrome_intl = false;
                    let mut raw_man_made: Option<&str> = None;
                    let mut raw_tower_type: Option<&str> = None;

                    for (key, value) in way.tags() {
                        match key {
                            "highway" => road_class = canonical_road_class(value),
                            "waterway" => waterway_class = canonical_waterway_class(value),
                            "building" => building_class = canonical_building_class(value),
                            "natural" | "landuse" => {
                                if tree_class.is_none() {
                                    tree_class = canonical_tree_class(key, value);
                                }
                                if industrial_class.is_none() {
                                    industrial_class = canonical_industrial_class(key, value);
                                }
                            }
                            "military" => military_class = canonical_military_class(key, value),
                            "power" => raw_power = Some(value),
                            "voltage" => raw_voltage = Some(value),
                            "railway" => raw_railway = canonical_railway_class(value),
                            "man_made" => {
                                if value == "pipeline" {
                                    raw_pipeline = true;
                                } else {
                                    if industrial_class.is_none() {
                                        industrial_class = canonical_industrial_class(key, value);
                                    }
                                    raw_man_made = Some(value);
                                    if port_class.is_none() {
                                        port_class = canonical_port_class(key, value);
                                    }
                                    if surv_class.is_none() {
                                        surv_class = canonical_surv_class(key, value);
                                    }
                                }
                            }
                            "substance" => raw_substance = Some(value),
                            "aeroway" => raw_aeroway = Some(value),
                            "aerodrome:type" => {
                                if value == "international" {
                                    raw_aerodrome_intl = true;
                                }
                            }
                            "tower:type" => raw_tower_type = Some(value),
                            "amenity" => {
                                if govt_class.is_none() {
                                    govt_class = canonical_govt_class(key, value);
                                }
                                if port_class.is_none() {
                                    port_class = canonical_port_class(key, value);
                                }
                            }
                            "office" | "government" => {
                                if govt_class.is_none() {
                                    govt_class = canonical_govt_class(key, value);
                                }
                            }
                            "harbour" | "leisure" => {
                                if port_class.is_none() {
                                    port_class = canonical_port_class(key, value);
                                }
                            }
                            "surveillance" => {
                                if surv_class.is_none() {
                                    surv_class = canonical_surv_class(key, value);
                                }
                            }
                            "name" if raw_name.is_none() => raw_name = Some(value),
                            _ => {}
                        }
                    }

                    let voltage_kv = raw_voltage.and_then(parse_voltage_kv);
                    let power_class = raw_power.and_then(|pt| canonical_power_class(pt, voltage_kv));
                    let pipeline_class = if raw_pipeline {
                        Some(canonical_pipeline_class(raw_substance.unwrap_or("")))
                    } else {
                        None
                    };
                    let aeroway_class =
                        raw_aeroway.and_then(|a| canonical_aeroway_class(a, raw_aerodrome_intl));
                    let comm_class =
                        raw_man_made.and_then(|m| canonical_comm_class(m, raw_tower_type));

                    let any_match = (command.build_roads && road_class.is_some())
                        || (command.build_waterways && waterway_class.is_some())
                        || (command.build_buildings && building_class.is_some())
                        || (command.build_trees && tree_class.is_some())
                        || (command.build_power && power_class.is_some())
                        || (command.build_rail && raw_railway.is_some())
                        || (command.build_pipeline && pipeline_class.is_some())
                        || (command.build_aeroway && aeroway_class.is_some())
                        || (command.build_military && military_class.is_some())
                        || (command.build_comm && comm_class.is_some())
                        || (command.build_industrial && industrial_class.is_some())
                        || (command.build_port && port_class.is_some())
                        || (command.build_government && govt_class.is_some())
                        || (command.build_surveillance && surv_class.is_some());
                    if !any_match {
                        continue;
                    }

                    points_buf.clear();
                    for id in way.refs() {
                        if let Some(&pt) = node_map.get(&id) {
                            points_buf.push(pt);
                        }
                    }
                    if points_buf.len() < 2 {
                        continue;
                    }

                    let way_bounds = polyline_bounds(&points_buf);
                    if !bounds_intersect(way_bounds, bounds) {
                        continue;
                    }

                    matched.push(MatchedBboxWay {
                        way_id: way.id(),
                        way_bounds,
                        points: points_buf.clone(),
                        name: raw_name.map(str::to_owned),
                        road_class,
                        waterway_class,
                        building_class,
                        tree_class,
                        military_class,
                        industrial_class,
                        port_class,
                        govt_class,
                        surv_class,
                        power_class,
                        raw_railway,
                        pipeline_class,
                        aeroway_class,
                        comm_class,
                    });
                }
                Ok(matched)
            })
            .collect::<Result<Vec<_>, _>>()?;

        macro_rules! push_bbox_feature {
            ($enabled:expr, $cls:expr, $seen:expr, $map:expr, $is_poly:expr, $w:expr) => {
                if $enabled {
                    if let Some(cls) = $cls {
                        if $seen.insert($w.way_id) {
                            let mut cells = focus_cells_iter($w.way_bounds).peekable();
                            let feature = WayFeature {
                                way_id: $w.way_id,
                                feature_class: Cow::Borrowed(cls),
                                name: $w.name.clone(),
                                points: $w.points.clone(),
                                is_polygon: $is_poly,
                            };
                            while let Some(cell) = cells.next() {
                                touched_cells.insert(cell);
                                if cells.peek().is_none() {
                                    $map.entry(cell).or_default().push(feature);
                                    break;
                                } else {
                                    $map.entry(cell).or_default().push(feature.clone());
                                }
                            }
                            feature_count += 1;
                            buffered += 1;
                        }
                    }
                }
            };
        }

        for batch_ways in partials {
            for w in batch_ways {
                scanned_ways += 1;

                if command.build_roads {
                    if let Some(cls) = w.road_class {
                        if seen_road_ids.insert(w.way_id) {
                            let mut cells = focus_cells_iter(w.way_bounds).peekable();
                            let road = RoadPolyline {
                                way_id: w.way_id,
                                road_class: Cow::Borrowed(cls),
                                name: w.name.clone(),
                                points: w.points.clone(),
                            };
                            while let Some(cell) = cells.next() {
                                touched_cells.insert(cell);
                                if cells.peek().is_none() {
                                    roads_by_cell.entry(cell).or_default().push(road);
                                    break;
                                } else {
                                    roads_by_cell.entry(cell).or_default().push(road.clone());
                                }
                            }
                            feature_count += 1;
                            buffered += 1;
                        }
                    }
                }

                push_bbox_feature!(command.build_waterways, w.waterway_class, seen_waterway_ids, waterways_by_cell, false, w);
                push_bbox_feature!(command.build_buildings, w.building_class, seen_building_ids, buildings_by_cell, true, w);
                push_bbox_feature!(command.build_trees, w.tree_class, seen_tree_ids, trees_by_cell, true, w);
                push_bbox_feature!(
                    command.build_power,
                    w.power_class,
                    seen_power_ids,
                    power_by_cell,
                    matches!(w.power_class, Some("substation" | "power_plant")),
                    w
                );
                push_bbox_feature!(command.build_rail, w.raw_railway, seen_rail_ids, rail_by_cell, false, w);
                push_bbox_feature!(command.build_pipeline, w.pipeline_class, seen_pipeline_ids, pipeline_by_cell, false, w);
                push_bbox_feature!(
                    command.build_aeroway,
                    w.aeroway_class,
                    seen_aeroway_ids,
                    aeroway_by_cell,
                    matches!(w.aeroway_class, Some("intl_airport" | "dom_airport" | "airfield" | "airstrip" | "terminal")),
                    w
                );
                push_bbox_feature!(command.build_military, w.military_class, seen_military_ids, military_by_cell, true, w);
                push_bbox_feature!(command.build_comm, w.comm_class, seen_comm_ids, comm_by_cell, false, w);
                push_bbox_feature!(
                    command.build_industrial,
                    w.industrial_class,
                    seen_industrial_ids,
                    industrial_by_cell,
                    matches!(w.industrial_class, Some("industrial" | "mine")),
                    w
                );
                push_bbox_feature!(
                    command.build_port,
                    w.port_class,
                    seen_port_ids,
                    port_by_cell,
                    matches!(w.port_class, Some("harbour" | "marina" | "shipyard")),
                    w
                );
                push_bbox_feature!(command.build_government, w.govt_class, seen_govt_ids, govt_by_cell, true, w);
                push_bbox_feature!(command.build_surveillance, w.surv_class, seen_surv_ids, surv_by_cell, false, w);
            }
        }

        if buffered >= ROAD_FLUSH_THRESHOLD {
            written_cells += flush_all_chunks(
                &command.cache_dir,
                &mut roads_by_cell,
                &mut waterways_by_cell,
                &mut buildings_by_cell,
                &mut trees_by_cell,
                &mut power_by_cell,
                &mut rail_by_cell,
                &mut pipeline_by_cell,
                &mut aeroway_by_cell,
                &mut military_by_cell,
                &mut comm_by_cell,
                &mut industrial_by_cell,
                &mut port_by_cell,
                &mut govt_by_cell,
                &mut surv_by_cell,
                srtm.as_mut().map(|s| &mut **s),
            )?;
            buffered = 0;

            // Checkpoint: file position of the next blob to read.
            let checkpoint = pos.load(Ordering::Relaxed);
            candidate_nodes.save_scan_offset("way_scan", checkpoint)?;

            progress(RoadBuildProgress {
                metrics: None,
                stage: "Writing Cache".to_owned(),
                fraction: 0.76,
                message: format!(
                    "Flushed features to {} cache files so far ({} features)",
                    written_cells, feature_count
                ),
            });
        }

        if scanned_ways >= next_progress_ways {
            progress(RoadBuildProgress {
                metrics: None,
                stage: "Scanning Ways".to_owned(),
                fraction: 0.75,
                message: format!(
                    "Scanned {} ways; kept {} features across {} cells",
                    scanned_ways,
                    feature_count,
                    touched_cells.len()
                ),
            });
            while scanned_ways >= next_progress_ways {
                next_progress_ways += 250_000;
            }
        }
    }

    written_cells += flush_all_chunks(
        &command.cache_dir,
        &mut roads_by_cell,
        &mut waterways_by_cell,
        &mut buildings_by_cell,
        &mut trees_by_cell,
        &mut power_by_cell,
        &mut rail_by_cell,
        &mut pipeline_by_cell,
        &mut aeroway_by_cell,
        &mut military_by_cell,
        &mut comm_by_cell,
        &mut industrial_by_cell,
        &mut port_by_cell,
        &mut govt_by_cell,
        &mut surv_by_cell,
        srtm.as_mut().map(|s| &mut **s),
    )?;

    // Way scan finished — clear checkpoint so future runs start fresh.
    candidate_nodes.clear_scan_offset("way_scan")?;

    Ok(FeatureBuildStats {
        feature_count,
        cell_count: touched_cells.len(),
        written_cells,
    })
}

fn flush_all_chunks(
    cache_dir: &Path,
    roads_by_cell: &mut HashMap<(i32, i32), Vec<RoadPolyline>>,
    waterways_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    buildings_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    trees_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    power_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    rail_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    pipeline_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    aeroway_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    military_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    comm_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    industrial_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    port_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    govt_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    surv_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    mut srtm: Option<&mut SrtmSampler>,
) -> Result<usize, String> {
    let mut total = 0usize;

    macro_rules! flush_feature {
        ($map:expr, $prefix:expr) => {
            if !$map.is_empty() {
                let batch = std::mem::take($map);
                total += merge_write_feature_cells(
                    cache_dir,
                    $prefix,
                    &batch,
                    srtm.as_mut().map(|s| &mut **s),
                )?;
            }
        };
    }

    if !roads_by_cell.is_empty() {
        let batch = std::mem::take(roads_by_cell);
        total += merge_write_cells(cache_dir, &batch, srtm.as_mut().map(|s| &mut **s))?;
    }
    flush_feature!(waterways_by_cell, "waterway");
    flush_feature!(buildings_by_cell, "building");
    flush_feature!(trees_by_cell, "tree");
    flush_feature!(power_by_cell, "power");
    flush_feature!(rail_by_cell, "railway");
    flush_feature!(pipeline_by_cell, "pipeline");
    flush_feature!(aeroway_by_cell, "aeroway");
    flush_feature!(military_by_cell, "military");
    flush_feature!(comm_by_cell, "comm");
    flush_feature!(industrial_by_cell, "industrial");
    flush_feature!(port_by_cell, "port");
    flush_feature!(govt_by_cell, "government");
    flush_feature!(surv_by_cell, "surveillance");

    Ok(total)
}

fn candidate_node_store_path(cache_dir: &Path, bounds: GeoBounds) -> PathBuf {
    cache_dir.join(".builder_state").join(format!(
        "candidate_nodes_all_{:+08.4}_{:+08.4}_{:+09.4}_{:+09.4}.sqlite",
        bounds.min_lat, bounds.max_lat, bounds.min_lon, bounds.max_lon
    ))
}
