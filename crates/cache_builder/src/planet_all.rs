//! Planet-scale two-pass OSM processor.
//!
//! # Overview
//! Pass 1: stream all nodes to `tmp_dir/planet_nodes.bin` via `NodeWriter`.
//! Sort: call `sort_in_place` (skipped when planet PBF is already ordered).
//! Pass 2: stream all ways; resolve node refs via `NodeLookup`; write `.1kc`
//! cell files to `out_dir`.
//!
//! # Resumability
//! A plain-text checkpoint file (`tmp_dir/checkpoint.txt`) tracks:
//! - `pass1_offset` — PBF byte offset where Pass 1 was interrupted
//! - `pass1_record_count` — number of node records already written
//! - `pass1_complete` — true once Pass 1 + sort is done
//! - `pass2_offset` — PBF byte offset where Pass 2 was last checkpointed
//!
//! Interrupted runs restart from the checkpoint without re-sorting or
//! re-scanning already-written output cells.

use crate::args::PlanetAllCommand;
use crate::flat_node_store::{NodeLookup, NodeWriter, sort_in_place, RECORD_BYTES};
use crate::geojson::{ensure_cache_dir, merge_write_cells, merge_write_feature_cells};
use crate::roads::{open_planet_at, PosReader, RoadBuildProgress};
use crate::srtm::SrtmSampler;
use crate::util::{
    canonical_aeroway_class, canonical_building_class, canonical_comm_class,
    canonical_govt_class, canonical_industrial_class, canonical_military_class,
    canonical_pipeline_class, canonical_port_class, canonical_power_class,
    canonical_railway_class, canonical_road_class, canonical_surv_class,
    canonical_tree_class, canonical_waterway_class, focus_cells_for_bounds,
    parse_voltage_kv, polyline_bounds, GeoPoint, RoadPolyline, WayFeature,
};
use osmpbf::{BlobDecode, BlobReader};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

const FLUSH_THRESHOLD: usize = 100_000;

// ── Checkpoint ────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Checkpoint {
    pass1_offset: u64,
    pass1_record_count: u64,
    pass1_complete: bool,
    pass2_offset: u64,
}

impl Checkpoint {
    fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let mut cp = Self::default();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "pass1_offset" => cp.pass1_offset = v.trim().parse().unwrap_or(0),
                    "pass1_record_count" => {
                        cp.pass1_record_count = v.trim().parse().unwrap_or(0)
                    }
                    "pass1_complete" => cp.pass1_complete = v.trim() == "true",
                    "pass2_offset" => cp.pass2_offset = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        cp
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let text = format!(
            "pass1_offset={}\npass1_record_count={}\npass1_complete={}\npass2_offset={}\n",
            self.pass1_offset,
            self.pass1_record_count,
            self.pass1_complete,
            self.pass2_offset,
        );
        fs::write(path, text).map_err(|e| e.to_string())
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn build_planet_cache(cmd: PlanetAllCommand) -> Result<(), String> {
    build_planet_cache_with_progress(cmd, &mut |_| {}).map(|_| ())
}

pub fn build_planet_cache_with_progress(
    cmd: PlanetAllCommand,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<String, String> {
    if !cmd.planet_path.exists() {
        return Err(format!(
            "Planet file not found: {}",
            cmd.planet_path.display()
        ));
    }
    ensure_cache_dir(&cmd.out_dir)?;
    fs::create_dir_all(&cmd.tmp_dir).map_err(|e| e.to_string())?;

    let node_file = cmd.tmp_dir.join("planet_nodes.bin");
    let checkpoint_path = cmd.tmp_dir.join("checkpoint.txt");
    let sort_tmp = cmd.tmp_dir.join("sort_chunks");

    let mut cp = Checkpoint::load(&checkpoint_path);

    // ── Pass 1: collect all nodes ─────────────────────────────────────────────
    if !cp.pass1_complete {
        cp.pass1_record_count = run_pass1(
            &cmd.planet_path,
            &node_file,
            &mut cp,
            &checkpoint_path,
            progress,
        )?;

        progress(RoadBuildProgress {
            stage: "Sorting Nodes".to_owned(),
            fraction: 0.30,
            message: format!(
                "Sorting {} node records…",
                cp.pass1_record_count
            ),
        });

        sort_in_place(
            &node_file,
            &sort_tmp,
            cp.pass1_record_count,
            &mut |msg| {
                progress(RoadBuildProgress {
                    stage: "Sorting Nodes".to_owned(),
                    fraction: 0.35,
                    message: msg,
                });
            },
        )?;

        cp.pass1_complete = true;
        cp.pass1_offset = 0; // no longer needed
        cp.save(&checkpoint_path)?;

        progress(RoadBuildProgress {
            stage: "Sorted Nodes".to_owned(),
            fraction: 0.40,
            message: format!(
                "Pass 1 complete: {} nodes ready for lookup",
                cp.pass1_record_count
            ),
        });
    } else {
        // Derive record count from file size if checkpoint predates the field.
        if cp.pass1_record_count == 0 {
            cp.pass1_record_count = fs::metadata(&node_file)
                .map(|m| m.len() / RECORD_BYTES)
                .unwrap_or(0);
        }
        progress(RoadBuildProgress {
            stage: "Loaded Node Store".to_owned(),
            fraction: 0.40,
            message: format!(
                "Pass 1 already complete ({} nodes)",
                cp.pass1_record_count
            ),
        });
    }

    // ── Pass 2: scan ways ─────────────────────────────────────────────────────
    let node_lookup = Arc::new(NodeLookup::open(&node_file, cp.pass1_record_count)?);

    progress(RoadBuildProgress {
        stage: "Scanning Ways".to_owned(),
        fraction: 0.41,
        message: "Opening node lookup; starting Pass 2…".to_owned(),
    });

    let stats = run_pass2(&cmd, &node_lookup, &mut cp, &checkpoint_path, progress)?;

    if cmd.build_admin {
        progress(RoadBuildProgress {
            stage: "Admin Skipped".to_owned(),
            fraction: 0.98,
            message: "Note: admin boundaries are not yet supported in planet-all mode.".to_owned(),
        });
    }

    let summary = format!(
        "planet-all: {} features across {} cells; {} cache files written to {}",
        stats.feature_count,
        stats.cell_count,
        stats.written_cells,
        cmd.out_dir.display()
    );
    progress(RoadBuildProgress {
        stage: "Completed".to_owned(),
        fraction: 1.0,
        message: summary.clone(),
    });
    Ok(summary)
}

// ── Pass 1 ────────────────────────────────────────────────────────────────────

fn run_pass1(
    planet_path: &Path,
    node_file: &Path,
    cp: &mut Checkpoint,
    checkpoint_path: &Path,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<u64, String> {
    let resume = cp.pass1_offset;

    let mut writer = if resume == 0 {
        NodeWriter::create(node_file)?
    } else {
        progress(RoadBuildProgress {
            stage: "Resuming Pass 1".to_owned(),
            fraction: 0.05,
            message: format!(
                "Resuming Pass 1 from offset {resume} ({} nodes so far)",
                cp.pass1_record_count
            ),
        });
        NodeWriter::append(node_file)?
    };

    let (reader, pos) = open_planet_at(planet_path, resume)?;
    let mut scanned = 0u64;

    for blob_result in reader {
        let blob = blob_result.map_err(|e| e.to_string())?;
        let decoded = blob.decode().map_err(|e| e.to_string())?;
        let BlobDecode::OsmData(block) = decoded else {
            continue;
        };

        for element in block.elements() {
            let (id, lat, lon) = match element {
                osmpbf::Element::Node(n) => (n.id(), n.lat() as f32, n.lon() as f32),
                osmpbf::Element::DenseNode(n) => (n.id(), n.lat() as f32, n.lon() as f32),
                _ => continue,
            };
            writer.write(id, lat, lon)?;
            scanned += 1;
        }

        // Checkpoint every ~5M nodes.
        if scanned % 5_000_000 < 4096 {
            let offset = pos.load(Ordering::Relaxed);
            cp.pass1_offset = offset;
            cp.pass1_record_count = writer.count;
            cp.save(checkpoint_path)?;

            progress(RoadBuildProgress {
                stage: "Collecting Nodes".to_owned(),
                fraction: 0.20,
                message: format!(
                    "Pass 1: {} nodes written ({:.1} GB to disk)",
                    writer.count,
                    writer.count as f64 * RECORD_BYTES as f64 / 1e9
                ),
            });
        }
    }

    let total = writer.count;
    writer.finish()?;
    Ok(total)
}

// ── Pass 2 ────────────────────────────────────────────────────────────────────

struct BuildStats {
    feature_count: usize,
    cell_count: usize,
    written_cells: usize,
}

fn run_pass2(
    cmd: &PlanetAllCommand,
    node_lookup: &Arc<NodeLookup>,
    cp: &mut Checkpoint,
    checkpoint_path: &Path,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<BuildStats, String> {
    let resume = cp.pass2_offset;
    if resume > 0 {
        progress(RoadBuildProgress {
            stage: "Resuming Pass 2".to_owned(),
            fraction: 0.42,
            message: format!("Resuming Pass 2 from offset {resume}"),
        });
    }

    let (reader, pos) = open_planet_at(&cmd.planet_path, resume)?;

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

    let mut scanned_ways = 0usize;
    let mut feature_count = 0usize;
    let mut cell_set: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
    let mut written_cells = 0usize;
    let mut buffered = 0usize;
    let mut srtm = cmd.srtm_root.clone().map(SrtmSampler::new);

    for blob_result in reader {
        let blob = blob_result.map_err(|e| e.to_string())?;
        let decoded = blob.decode().map_err(|e| e.to_string())?;
        let BlobDecode::OsmData(block) = decoded else {
            continue;
        };

        for element in block.elements() {
            let osmpbf::Element::Way(way) = element else {
                continue;
            };

            let mut road_class: Option<&'static str> = None;
            let mut waterway_class: Option<&'static str> = None;
            let mut building_class: Option<&'static str> = None;
            let mut tree_class: Option<&'static str> = None;
            let mut military_class: Option<&'static str> = None;
            let mut industrial_class: Option<&'static str> = None;
            let mut port_class: Option<&'static str> = None;
            let mut govt_class: Option<&'static str> = None;
            let mut surv_class: Option<&'static str> = None;
            let mut name: Option<String> = None;

            let mut raw_power: Option<String> = None;
            let mut raw_voltage: Option<String> = None;
            let mut raw_railway: Option<&'static str> = None;
            let mut raw_pipeline = false;
            let mut raw_substance: Option<String> = None;
            let mut raw_aeroway: Option<String> = None;
            let mut raw_aerodrome_intl = false;
            let mut raw_man_made: Option<String> = None;
            let mut raw_tower_type: Option<String> = None;

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
                    "power" => raw_power = Some(value.to_owned()),
                    "voltage" => raw_voltage = Some(value.to_owned()),
                    "railway" => raw_railway = canonical_railway_class(value),
                    "man_made" => {
                        if value == "pipeline" {
                            raw_pipeline = true;
                        } else {
                            if industrial_class.is_none() {
                                industrial_class = canonical_industrial_class(key, value);
                            }
                            raw_man_made = Some(value.to_owned());
                            if port_class.is_none() {
                                port_class = canonical_port_class(key, value);
                            }
                            if surv_class.is_none() {
                                surv_class = canonical_surv_class(key, value);
                            }
                        }
                    }
                    "substance" => raw_substance = Some(value.to_owned()),
                    "aeroway" => raw_aeroway = Some(value.to_owned()),
                    "aerodrome:type" => {
                        if value == "international" {
                            raw_aerodrome_intl = true;
                        }
                    }
                    "tower:type" => raw_tower_type = Some(value.to_owned()),
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
                    "name" if name.is_none() => name = Some(value.to_owned()),
                    _ => {}
                }
            }

            let voltage_kv = raw_voltage.as_deref().and_then(parse_voltage_kv);
            let power_class =
                raw_power.as_deref().and_then(|pt| canonical_power_class(pt, voltage_kv));
            let pipeline_class: Option<&'static str> = if raw_pipeline {
                Some(canonical_pipeline_class(raw_substance.as_deref().unwrap_or("")))
            } else {
                None
            };
            let aeroway_class = raw_aeroway
                .as_deref()
                .and_then(|a| canonical_aeroway_class(a, raw_aerodrome_intl));
            let comm_class = raw_man_made
                .as_deref()
                .and_then(|m| canonical_comm_class(m, raw_tower_type.as_deref()));

            let any_match = (cmd.build_roads && road_class.is_some())
                || (cmd.build_waterways && waterway_class.is_some())
                || (cmd.build_buildings && building_class.is_some())
                || (cmd.build_trees && tree_class.is_some())
                || (cmd.build_power && power_class.is_some())
                || (cmd.build_rail && raw_railway.is_some())
                || (cmd.build_pipeline && pipeline_class.is_some())
                || (cmd.build_aeroway && aeroway_class.is_some())
                || (cmd.build_military && military_class.is_some())
                || (cmd.build_comm && comm_class.is_some())
                || (cmd.build_industrial && industrial_class.is_some())
                || (cmd.build_port && port_class.is_some())
                || (cmd.build_government && govt_class.is_some())
                || (cmd.build_surveillance && surv_class.is_some());
            if !any_match {
                continue;
            }

            // Resolve node refs via the flat binary lookup.
            let points: Vec<GeoPoint> = way
                .refs()
                .filter_map(|id| {
                    node_lookup
                        .lookup(id)
                        .map(|(lat, lon)| GeoPoint { lat, lon })
                })
                .collect();
            if points.len() < 2 {
                continue;
            }

            let way_bounds = polyline_bounds(&points);
            scanned_ways += 1;

            macro_rules! emit_feature {
                ($enabled:expr, $cls:expr, $map:expr, $is_poly:expr) => {
                    if $enabled {
                        if let Some(cls) = $cls {
                            let feature = WayFeature {
                                way_id: way.id(),
                                feature_class: cls.to_owned(),
                                name: name.clone(),
                                points: points.clone(),
                                is_polygon: $is_poly,
                            };
                            let mut assigned = std::collections::HashSet::new();
                            for cell in focus_cells_for_bounds(way_bounds) {
                                if assigned.insert(cell) {
                                    cell_set.insert(cell);
                                    $map.entry(cell).or_default().push(feature.clone());
                                }
                            }
                            feature_count += 1;
                            buffered += 1;
                        }
                    }
                };
            }

            if cmd.build_roads {
                if let Some(cls) = road_class {
                    let road = RoadPolyline {
                        way_id: way.id(),
                        road_class: cls.to_owned(),
                        name: name.clone(),
                        points: points.clone(),
                    };
                    let mut assigned = std::collections::HashSet::new();
                    for cell in focus_cells_for_bounds(way_bounds) {
                        if assigned.insert(cell) {
                            cell_set.insert(cell);
                            roads_by_cell.entry(cell).or_default().push(road.clone());
                        }
                    }
                    feature_count += 1;
                    buffered += 1;
                }
            }

            emit_feature!(cmd.build_waterways, waterway_class, waterways_by_cell, false);
            emit_feature!(cmd.build_buildings, building_class, buildings_by_cell, true);
            emit_feature!(cmd.build_trees, tree_class, trees_by_cell, true);
            emit_feature!(cmd.build_power, power_class, power_by_cell, {
                matches!(power_class, Some(c) if c == "substation" || c == "power_plant")
            });
            emit_feature!(cmd.build_rail, raw_railway, rail_by_cell, false);
            emit_feature!(cmd.build_pipeline, pipeline_class, pipeline_by_cell, false);
            emit_feature!(cmd.build_aeroway, aeroway_class, aeroway_by_cell, {
                matches!(aeroway_class, Some(c)
                    if matches!(c, "intl_airport" | "dom_airport" | "airfield" | "airstrip" | "terminal"))
            });
            emit_feature!(cmd.build_military, military_class, military_by_cell, true);
            emit_feature!(cmd.build_comm, comm_class, comm_by_cell, false);
            emit_feature!(cmd.build_industrial, industrial_class, industrial_by_cell, {
                matches!(industrial_class, Some(c) if c == "industrial" || c == "mine")
            });
            emit_feature!(cmd.build_port, port_class, port_by_cell, {
                matches!(port_class, Some(c)
                    if c == "harbour" || c == "marina" || c == "shipyard")
            });
            emit_feature!(cmd.build_government, govt_class, govt_by_cell, true);
            emit_feature!(cmd.build_surveillance, surv_class, surv_by_cell, false);

            if buffered >= FLUSH_THRESHOLD {
                written_cells += flush_all(
                    &cmd.out_dir,
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
                    srtm.as_mut(),
                )?;
                buffered = 0;

                cp.pass2_offset = pos.load(Ordering::Relaxed);
                cp.save(checkpoint_path)?;

                if scanned_ways % 500_000 < FLUSH_THRESHOLD {
                    progress(RoadBuildProgress {
                        stage: "Scanning Ways".to_owned(),
                        fraction: 0.75,
                        message: format!(
                            "Pass 2: {scanned_ways} ways scanned; {feature_count} features; \
                             {written_cells} cache files written",
                        ),
                    });
                }
            }
        }
    }

    written_cells += flush_all(
        &cmd.out_dir,
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
        srtm.as_mut(),
    )?;

    cp.pass2_offset = 0; // complete — clear checkpoint
    cp.save(checkpoint_path)?;

    Ok(BuildStats {
        feature_count,
        cell_count: cell_set.len(),
        written_cells,
    })
}

// ── Flush helpers ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn flush_all(
    out_dir: &Path,
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
    srtm: Option<&mut SrtmSampler>,
) -> Result<usize, String> {
    let mut total = 0usize;
    // Thread srtm through each flush call.
    let srtm_ref = &mut srtm.map(|s| s as &mut SrtmSampler);

    macro_rules! flush_feature {
        ($map:expr, $prefix:expr) => {
            if !$map.is_empty() {
                let batch = std::mem::take($map);
                total += merge_write_feature_cells(
                    out_dir,
                    $prefix,
                    &batch,
                    srtm_ref.as_mut().map(|s| &mut **s),
                )?;
            }
        };
    }

    if !roads_by_cell.is_empty() {
        let batch = std::mem::take(roads_by_cell);
        total += merge_write_cells(out_dir, &batch, srtm_ref.as_mut().map(|s| &mut **s))?;
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
