use crate::args::BboxCommand;
use crate::geojson::{ensure_cache_dir, merge_write_cells, merge_write_feature_cells};
use crate::srtm::SrtmSampler;
use crate::node_store::NodeStore;
use crate::util::{
    GeoBounds, GeoPoint, RoadPolyline, WayFeature, bounds_intersect, canonical_road_class,
    canonical_waterway_class, canonical_building_class, canonical_tree_class,
    expand_bounds, focus_cells_for_bounds, point_in_bounds, polyline_bounds,
};
use osmpbf::{BlobDecode, BlobReader};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const ROAD_FLUSH_THRESHOLD: usize = 10_000;
const NODE_INSERT_BATCH:    usize = 50_000;

// ── Position-tracking reader ──────────────────────────────────────────────────
//
// Wraps a plain `File` and counts every byte delivered to the BlobReader.
// Because PBF blobs are length-prefixed and BlobReader reads exactly one blob
// per iterator step, `bytes_read` equals the file offset of the *start of the
// next* blob after each `next()` call — a safe resume point.

pub(crate) struct PosReader {
    inner: File,
    pos:   Arc<AtomicU64>,
}

impl Read for PosReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.pos.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

// `File` is Send; `Arc<AtomicU64>` is Send.
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
    let pos    = Arc::new(AtomicU64::new(start_offset));
    let reader = BlobReader::new(PosReader { inner: file, pos: pos.clone() });
    Ok((reader, pos))
}

// ── Public entry points ───────────────────────────────────────────────────────

pub fn build_bbox_cache(command: BboxCommand) -> Result<(), String> {
    build_bbox_cache_with_progress(command, &mut |_| {}).map(|_| ())
}

#[derive(Clone, Debug)]
pub struct RoadBuildProgress {
    pub stage:    String,
    pub fraction: f32,
    pub message:  String,
}

pub fn build_bbox_cache_with_progress(
    command:  BboxCommand,
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
    let expanded         = expand_bounds(bounds, command.margin_degrees);
    let node_store_path  = candidate_node_store_path(&command.cache_dir, bounds);

    progress(RoadBuildProgress {
        stage:    "Scanning Nodes".to_owned(),
        fraction: 0.02,
        message:  format!(
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
        stage:    "Scanning Ways".to_owned(),
        fraction: 0.40,
        message:  format!("Prepared {} candidate nodes", candidate_nodes.count()?),
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
            stage:    "Scanning Relations".to_owned(),
            fraction: 0.83,
            message:  "Scanning admin boundary relations…".to_owned(),
        });
        crate::admin::load_or_build_admin_boundaries(
            &command,
            bounds,
            &mut candidate_nodes,
            progress,
        )?;
    }

    progress(RoadBuildProgress {
        stage:    "Writing Cache".to_owned(),
        fraction: 0.82,
        message:  format!(
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
        stage:    "Completed".to_owned(),
        fraction: 1.0,
        message:  summary.clone(),
    });
    Ok(summary)
}

// ── Pass 1: candidate node collection ────────────────────────────────────────

fn load_or_collect_candidate_nodes(
    planet_path:     &Path,
    bounds:          GeoBounds,
    node_store_path: &Path,
    progress:        &mut dyn FnMut(RoadBuildProgress),
) -> Result<NodeStore, String> {
    let mut node_store = NodeStore::open(node_store_path)?;

    if node_store.is_complete()? {
        let node_count = node_store.count()?;
        progress(RoadBuildProgress {
            stage:    "Loaded Node Cache".to_owned(),
            fraction: 0.32,
            message:  format!(
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
            stage:    "Resuming Node Scan".to_owned(),
            fraction: 0.03,
            message:  format!(
                "Resuming node scan from file offset {} ({} nodes collected so far)",
                resume_offset.unwrap(),
                node_store.count()?
            ),
        });
    }

    collect_candidate_nodes(planet_path, bounds, &mut node_store, resume_offset, progress)?;
    node_store.mark_complete()?;
    node_store.clear_scan_offset("node_scan")?;

    let node_count = node_store.count()?;
    progress(RoadBuildProgress {
        stage:    "Saved Node Cache".to_owned(),
        fraction: 0.35,
        message:  format!("Saved {} candidate nodes to {}", node_count, node_store_path.display()),
    });
    Ok(node_store)
}

fn collect_candidate_nodes(
    planet_path:   &Path,
    bounds:        GeoBounds,
    node_store:    &mut NodeStore,
    resume_offset: Option<u64>,
    progress:      &mut dyn FnMut(RoadBuildProgress),
) -> Result<(), String> {
    let start = resume_offset.unwrap_or(0);
    let (reader, pos) = open_planet_at(planet_path, start)?;

    let mut scanned = 0usize;
    let mut kept    = 0usize;
    let mut batch: Vec<(i64, GeoPoint)> = Vec::with_capacity(NODE_INSERT_BATCH);

    for blob_result in reader {
        let blob    = blob_result.map_err(|e| e.to_string())?;
        let decoded = blob.decode().map_err(|e| e.to_string())?;
        let BlobDecode::OsmData(block) = decoded else { continue };

        for element in block.elements() {
            let (node_id, lat, lon) = match element {
                osmpbf::Element::Node(n)      => (n.id(), n.lat() as f32, n.lon() as f32),
                osmpbf::Element::DenseNode(n) => (n.id(), n.lat() as f32, n.lon() as f32),
                _ => continue,
            };
            let point = GeoPoint { lat, lon };
            if point_in_bounds(point, bounds) {
                batch.push((node_id, point));
                kept += 1;
            }
            scanned += 1;
        }

        // Flush + checkpoint after each full batch.
        if batch.len() >= NODE_INSERT_BATCH {
            node_store.insert_batch(&batch)?;
            batch.clear();
            // Save the position of the NEXT blob — safe to resume from here.
            let checkpoint = pos.load(Ordering::Relaxed);
            node_store.save_scan_offset("node_scan", checkpoint)?;

            if scanned % 2_000_000 < NODE_INSERT_BATCH {
                progress(RoadBuildProgress {
                    stage:    "Scanning Nodes".to_owned(),
                    fraction: 0.30,
                    message:  format!("Scanned {} elements; kept {} candidate nodes", scanned, kept),
                });
            }
        }
    }

    // Flush final partial batch (no checkpoint needed — mark_complete() follows).
    node_store.insert_batch(&batch)?;
    Ok(())
}

// ── Pass 2: combined feature collection by cell ───────────────────────────────

struct FeatureBuildStats {
    feature_count: usize,
    cell_count:    usize,
    written_cells: usize,
}

fn collect_all_features_by_cell(
    command:         &BboxCommand,
    bounds:          GeoBounds,
    candidate_nodes: &mut NodeStore,
    mut srtm:        Option<&mut SrtmSampler>,
    progress:        &mut dyn FnMut(RoadBuildProgress),
) -> Result<FeatureBuildStats, String> {
    // Resume from the last way-scan checkpoint if available.
    let resume_offset = candidate_nodes.get_scan_offset("way_scan")?;
    if let Some(off) = resume_offset {
        progress(RoadBuildProgress {
            stage:    "Resuming Way Scan".to_owned(),
            fraction: 0.41,
            message:  format!("Resuming way scan from file offset {off}"),
        });
    }

    let start         = resume_offset.unwrap_or(0);
    let (reader, pos) = open_planet_at(&command.planet_path, start)?;

    // Per-feature-type accumulators
    let mut seen_road_ids:      HashSet<i64> = HashSet::new();
    let mut seen_waterway_ids:  HashSet<i64> = HashSet::new();
    let mut seen_building_ids:  HashSet<i64> = HashSet::new();
    let mut seen_tree_ids:      HashSet<i64> = HashSet::new();

    let mut roads_by_cell:      HashMap<(i32, i32), Vec<RoadPolyline>> = HashMap::new();
    let mut waterways_by_cell:  HashMap<(i32, i32), Vec<WayFeature>>   = HashMap::new();
    let mut buildings_by_cell:  HashMap<(i32, i32), Vec<WayFeature>>   = HashMap::new();
    let mut trees_by_cell:      HashMap<(i32, i32), Vec<WayFeature>>   = HashMap::new();

    let mut scanned_ways  = 0usize;
    let mut feature_count = 0usize;
    let mut touched_cells: HashSet<(i32, i32)> = HashSet::new();
    let mut written_cells = 0usize;
    let mut buffered      = 0usize;

    for blob_result in reader {
        let blob    = blob_result.map_err(|e| e.to_string())?;
        let decoded = blob.decode().map_err(|e| e.to_string())?;
        let BlobDecode::OsmData(block) = decoded else { continue };

        for element in block.elements() {
            let osmpbf::Element::Way(way) = element else { continue };

            let mut road_class:     Option<&'static str> = None;
            let mut waterway_class: Option<&'static str> = None;
            let mut building_class: Option<&'static str> = None;
            let mut tree_class:     Option<&'static str> = None;
            let mut name:           Option<String>       = None;

            for (key, value) in way.tags() {
                if key == "highway" {
                    road_class = canonical_road_class(value);
                } else if key == "waterway" {
                    waterway_class = canonical_waterway_class(value);
                } else if key == "building" {
                    building_class = canonical_building_class(value);
                } else if key == "natural" || key == "landuse" {
                    tree_class = canonical_tree_class(key, value);
                } else if key == "name" && name.is_none() {
                    name = Some(value.to_owned());
                }
            }

            // Skip entirely if nothing matched
            let any_match = (command.build_roads      && road_class.is_some())
                         || (command.build_waterways   && waterway_class.is_some())
                         || (command.build_buildings   && building_class.is_some())
                         || (command.build_trees       && tree_class.is_some());
            if !any_match { continue; }

            let refs: Vec<_> = way.refs().collect();
            let points = candidate_nodes.points_for_refs(&refs).unwrap_or_default();
            if points.len() < 2 { continue; }

            let way_bounds = polyline_bounds(&points);
            if !bounds_intersect(way_bounds, bounds) { continue; }

            scanned_ways += 1;

            // Roads
            if command.build_roads {
                if let Some(cls) = road_class {
                    if seen_road_ids.insert(way.id()) {
                        let road = RoadPolyline {
                            way_id:     way.id(),
                            road_class: cls.to_owned(),
                            name:       name.clone(),
                            points:     points.clone(),
                        };
                        let mut assigned = HashSet::new();
                        for cell in focus_cells_for_bounds(way_bounds) {
                            if assigned.insert(cell) {
                                touched_cells.insert(cell);
                                roads_by_cell.entry(cell).or_default().push(road.clone());
                            }
                        }
                        feature_count += 1;
                        buffered      += 1;
                    }
                }
            }

            // Waterways
            if command.build_waterways {
                if let Some(cls) = waterway_class {
                    if seen_waterway_ids.insert(way.id()) {
                        let feature = WayFeature {
                            way_id:        way.id(),
                            feature_class: cls.to_owned(),
                            name:          name.clone(),
                            points:        points.clone(),
                            is_polygon:    false,
                        };
                        let mut assigned = HashSet::new();
                        for cell in focus_cells_for_bounds(way_bounds) {
                            if assigned.insert(cell) {
                                touched_cells.insert(cell);
                                waterways_by_cell.entry(cell).or_default().push(feature.clone());
                            }
                        }
                        feature_count += 1;
                        buffered      += 1;
                    }
                }
            }

            // Buildings
            if command.build_buildings {
                if let Some(cls) = building_class {
                    if seen_building_ids.insert(way.id()) {
                        let feature = WayFeature {
                            way_id:        way.id(),
                            feature_class: cls.to_owned(),
                            name:          name.clone(),
                            points:        points.clone(),
                            is_polygon:    true,
                        };
                        let mut assigned = HashSet::new();
                        for cell in focus_cells_for_bounds(way_bounds) {
                            if assigned.insert(cell) {
                                touched_cells.insert(cell);
                                buildings_by_cell.entry(cell).or_default().push(feature.clone());
                            }
                        }
                        feature_count += 1;
                        buffered      += 1;
                    }
                }
            }

            // Trees / forests
            if command.build_trees {
                if let Some(cls) = tree_class {
                    if seen_tree_ids.insert(way.id()) {
                        let feature = WayFeature {
                            way_id:        way.id(),
                            feature_class: cls.to_owned(),
                            name:          name.clone(),
                            points:        points.clone(),
                            is_polygon:    true,
                        };
                        let mut assigned = HashSet::new();
                        for cell in focus_cells_for_bounds(way_bounds) {
                            if assigned.insert(cell) {
                                touched_cells.insert(cell);
                                trees_by_cell.entry(cell).or_default().push(feature.clone());
                            }
                        }
                        feature_count += 1;
                        buffered      += 1;
                    }
                }
            }

            if buffered >= ROAD_FLUSH_THRESHOLD {
                written_cells += flush_all_chunks(
                    &command.cache_dir,
                    &mut roads_by_cell,
                    &mut waterways_by_cell,
                    &mut buildings_by_cell,
                    &mut trees_by_cell,
                    srtm.as_mut().map(|s| &mut **s),
                )?;
                buffered = 0;

                // Checkpoint: file position of the next blob to read.
                let checkpoint = pos.load(Ordering::Relaxed);
                candidate_nodes.save_scan_offset("way_scan", checkpoint)?;

                progress(RoadBuildProgress {
                    stage:    "Writing Cache".to_owned(),
                    fraction: 0.76,
                    message:  format!(
                        "Flushed features to {} cache files so far ({} features)",
                        written_cells, feature_count
                    ),
                });
            }

            if scanned_ways % 250_000 == 0 {
                progress(RoadBuildProgress {
                    stage:    "Scanning Ways".to_owned(),
                    fraction: 0.75,
                    message:  format!(
                        "Scanned {} ways; kept {} features across {} cells",
                        scanned_ways, feature_count, touched_cells.len()
                    ),
                });
            }
        }
    }

    written_cells += flush_all_chunks(
        &command.cache_dir,
        &mut roads_by_cell,
        &mut waterways_by_cell,
        &mut buildings_by_cell,
        &mut trees_by_cell,
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
    cache_dir:         &Path,
    roads_by_cell:     &mut HashMap<(i32, i32), Vec<RoadPolyline>>,
    waterways_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    buildings_by_cell: &mut HashMap<(i32, i32), Vec<WayFeature>>,
    trees_by_cell:     &mut HashMap<(i32, i32), Vec<WayFeature>>,
    mut srtm:          Option<&mut SrtmSampler>,
) -> Result<usize, String> {
    let mut total = 0usize;
    if !roads_by_cell.is_empty() {
        let batch = std::mem::take(roads_by_cell);
        total += merge_write_cells(cache_dir, &batch, srtm.as_mut().map(|s| &mut **s))?;
    }
    if !waterways_by_cell.is_empty() {
        let batch = std::mem::take(waterways_by_cell);
        total += merge_write_feature_cells(cache_dir, "waterway", &batch, srtm.as_mut().map(|s| &mut **s))?;
    }
    if !buildings_by_cell.is_empty() {
        let batch = std::mem::take(buildings_by_cell);
        total += merge_write_feature_cells(cache_dir, "building", &batch, srtm.as_mut().map(|s| &mut **s))?;
    }
    if !trees_by_cell.is_empty() {
        let batch = std::mem::take(trees_by_cell);
        total += merge_write_feature_cells(cache_dir, "tree", &batch, srtm.as_mut().map(|s| &mut **s))?;
    }
    Ok(total)
}

fn candidate_node_store_path(cache_dir: &Path, bounds: GeoBounds) -> PathBuf {
    cache_dir.join(".builder_state").join(format!(
        "candidate_nodes_all_{:+08.4}_{:+08.4}_{:+09.4}_{:+09.4}.sqlite",
        bounds.min_lat, bounds.max_lat, bounds.min_lon, bounds.max_lon
    ))
}
