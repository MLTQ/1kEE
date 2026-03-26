use crate::args::BboxCommand;
use crate::geojson::write_admin_level_file;
use crate::node_store::NodeStore;
use crate::roads::{RoadBuildProgress, open_planet_at};
use crate::util::{GeoBounds, GeoPoint};
use osmpbf::{BlobDecode, RelMemberType};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

const RELATION_BATCH: usize = 500;
const WAY_BATCH: usize = 1000;

/// Top-level entry point. Runs Pass R and Pass A if not yet complete, then stitches
/// rings and writes per-level GeoJSON files.
///
/// Returns the total number of GeoJSON Feature objects (rings) written.
pub fn load_or_build_admin_boundaries(
    command: &BboxCommand,
    _bounds: GeoBounds,
    node_store: &mut NodeStore,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<usize, String> {
    // Pass R: collect admin relations + member way IDs.
    if !node_store.is_relation_scan_complete()? {
        collect_admin_relations(&command.planet_path, node_store, progress)?;
    } else {
        let count = node_store.count_admin_relations()?;
        progress(RoadBuildProgress {
            stage: "Scanning Relations".to_owned(),
            fraction: 0.84,
            message: format!("Relation scan already complete ({count} relations cached)"),
        });
    }

    // Pass A: collect node refs for admin member ways.
    if !node_store.is_admin_way_scan_complete()? {
        collect_admin_way_nodes(&command.planet_path, node_store, progress)?;
    } else {
        progress(RoadBuildProgress {
            stage: "Scanning Admin Ways".to_owned(),
            fraction: 0.90,
            message: "Admin way scan already complete".to_owned(),
        });
    }

    // Stitch + write.
    stitch_and_write(&command.cache_dir, node_store, progress)
}

// ── Pass R: relation scan ─────────────────────────────────────────────────────

fn collect_admin_relations(
    planet_path: &std::path::Path,
    node_store: &mut NodeStore,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<(), String> {
    let resume_offset = node_store.get_scan_offset("relation_scan")?;
    if let Some(off) = resume_offset {
        progress(RoadBuildProgress {
            stage: "Scanning Relations".to_owned(),
            fraction: 0.83,
            message: format!("Resuming relation scan from file offset {off}"),
        });
    }

    let start = resume_offset.unwrap_or(0);
    let (reader, pos) = open_planet_at(planet_path, start)?;

    // Batch: (relation_id, name, admin_level, way_ids)
    let mut batch: Vec<(i64, Option<String>, u8, Vec<i64>)> = Vec::new();
    let mut total_relations = 0usize;

    for blob_result in reader {
        let blob = blob_result.map_err(|e| e.to_string())?;
        let decoded = blob.decode().map_err(|e| e.to_string())?;
        let BlobDecode::OsmData(block) = decoded else {
            continue;
        };

        for element in block.elements() {
            let osmpbf::Element::Relation(rel) = element else {
                continue;
            };

            // Check tags: boundary=administrative AND admin_level in {2,4,6,8}
            let mut is_boundary = false;
            let mut admin_level: Option<u8> = None;
            let mut name: Option<String> = None;

            for (key, value) in rel.tags() {
                match key {
                    "boundary" if value == "administrative" => is_boundary = true,
                    "admin_level" => {
                        admin_level = match value {
                            "2" => Some(2),
                            "4" => Some(4),
                            "6" => Some(6),
                            "8" => Some(8),
                            _ => None,
                        };
                    }
                    "name" if name.is_none() => name = Some(value.to_owned()),
                    _ => {}
                }
            }

            if !is_boundary {
                continue;
            }
            let Some(level) = admin_level else { continue };

            // Collect outer/empty-role member ways.
            let way_ids: Vec<i64> = rel
                .members()
                .filter_map(|m| {
                    if m.member_type != RelMemberType::Way {
                        return None;
                    }
                    let role = m.role().unwrap_or("");
                    if role == "inner" {
                        return None;
                    }
                    Some(m.member_id)
                })
                .collect();

            if way_ids.is_empty() {
                continue;
            }

            batch.push((rel.id(), name, level, way_ids));
            total_relations += 1;

            if batch.len() >= RELATION_BATCH {
                node_store.save_admin_relations_batch(&batch)?;
                batch.clear();
                let checkpoint = pos.load(Ordering::Relaxed);
                node_store.save_scan_offset("relation_scan", checkpoint)?;

                progress(RoadBuildProgress {
                    stage: "Scanning Relations".to_owned(),
                    fraction: 0.84,
                    message: format!("Collected {total_relations} admin relations so far"),
                });
            }
        }
    }

    // Flush remainder.
    if !batch.is_empty() {
        node_store.save_admin_relations_batch(&batch)?;
    }

    progress(RoadBuildProgress {
        stage: "Scanning Relations".to_owned(),
        fraction: 0.86,
        message: format!("Relation scan complete: {total_relations} admin relations found"),
    });

    node_store.mark_relation_scan_complete()?;
    node_store.clear_scan_offset("relation_scan")?;
    Ok(())
}

// ── Pass A: admin way node scan ───────────────────────────────────────────────

fn collect_admin_way_nodes(
    planet_path: &std::path::Path,
    node_store: &mut NodeStore,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<(), String> {
    let admin_way_ids: HashSet<i64> = node_store.get_admin_member_way_ids()?;

    if admin_way_ids.is_empty() {
        node_store.mark_admin_way_scan_complete()?;
        return Ok(());
    }

    let resume_offset = node_store.get_scan_offset("admin_way_scan")?;
    if let Some(off) = resume_offset {
        progress(RoadBuildProgress {
            stage: "Scanning Admin Ways".to_owned(),
            fraction: 0.87,
            message: format!("Resuming admin way scan from file offset {off}"),
        });
    }

    let start = resume_offset.unwrap_or(0);
    let (reader, pos) = open_planet_at(planet_path, start)?;

    let mut batch: Vec<(i64, Vec<i64>)> = Vec::new();
    let mut ways_found = 0usize;

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
            if !admin_way_ids.contains(&way.id()) {
                continue;
            }

            let node_ids: Vec<i64> = way.refs().collect();
            batch.push((way.id(), node_ids));
            ways_found += 1;

            if batch.len() >= WAY_BATCH {
                node_store.save_admin_way_nodes(&batch)?;
                batch.clear();
                let checkpoint = pos.load(Ordering::Relaxed);
                node_store.save_scan_offset("admin_way_scan", checkpoint)?;

                progress(RoadBuildProgress {
                    stage: "Scanning Admin Ways".to_owned(),
                    fraction: 0.89,
                    message: format!("Collected node refs for {ways_found} admin ways"),
                });
            }
        }
    }

    // Flush remainder.
    if !batch.is_empty() {
        node_store.save_admin_way_nodes(&batch)?;
    }

    progress(RoadBuildProgress {
        stage: "Scanning Admin Ways".to_owned(),
        fraction: 0.90,
        message: format!("Admin way scan complete: {ways_found} ways processed"),
    });

    node_store.mark_admin_way_scan_complete()?;
    node_store.clear_scan_offset("admin_way_scan")?;
    Ok(())
}

// ── Stitch + write ────────────────────────────────────────────────────────────

fn stitch_and_write(
    cache_dir: &std::path::Path,
    node_store: &NodeStore,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<usize, String> {
    let all_relations = node_store.get_admin_relation_ways()?;

    // Group by admin_level: level → Vec<(relation_id, name, rings)>
    let mut by_level: HashMap<u8, Vec<(i64, Option<String>, Vec<Vec<GeoPoint>>)>> = HashMap::new();

    for (relation_id, name, level, way_ids) in &all_relations {
        let way_map = node_store.get_way_coords_for_relation(way_ids)?;
        let rings = stitch_ways(&way_map);
        by_level
            .entry(*level)
            .or_default()
            .push((*relation_id, name.clone(), rings));
    }

    let mut total_written = 0usize;
    for (level, features) in &by_level {
        let count = write_admin_level_file(cache_dir, *level, features)?;
        total_written += count;
    }

    progress(RoadBuildProgress {
        stage: "Writing Admin Boundaries".to_owned(),
        fraction: 0.95,
        message: format!(
            "Wrote {total_written} admin boundary rings across {} levels",
            by_level.len()
        ),
    });

    Ok(total_written)
}

// ── Way stitching ─────────────────────────────────────────────────────────────

/// Greedy chain-stitching: tries to connect ways end-to-end using approximate
/// coordinate matching (rounded to 1e-5 degrees ≈ 1 m).
///
/// Returns a Vec of chains (each chain is a Vec<GeoPoint>).
pub fn stitch_ways(ways: &HashMap<i64, Vec<GeoPoint>>) -> Vec<Vec<GeoPoint>> {
    if ways.is_empty() {
        return Vec::new();
    }

    // Helper: approximate integer key for an endpoint.
    let key = |pt: GeoPoint| -> (i64, i64) { ((pt.lat * 1e5) as i64, (pt.lon * 1e5) as i64) };

    // Build an index: endpoint_key → way_id (or multiple).
    // We store (way_id, is_end) pairs so we can look up by either end.
    // endpoint_key → Vec<(way_id, reversed)>
    // reversed=false → key is the start; reversed=true → key is the end.
    let mut endpoint_index: HashMap<(i64, i64), Vec<i64>> = HashMap::new();
    for (&way_id, pts) in ways {
        if pts.len() < 2 {
            continue;
        }
        let start_key = key(*pts.first().unwrap());
        let end_key = key(*pts.last().unwrap());
        endpoint_index.entry(start_key).or_default().push(way_id);
        endpoint_index.entry(end_key).or_default().push(way_id);
    }

    let mut used: HashSet<i64> = HashSet::new();
    let mut chains: Vec<Vec<GeoPoint>> = Vec::new();

    for &start_way_id in ways.keys() {
        if used.contains(&start_way_id) {
            continue;
        }
        let start_pts = &ways[&start_way_id];
        if start_pts.len() < 2 {
            continue;
        }

        used.insert(start_way_id);
        let mut chain: Vec<GeoPoint> = start_pts.clone();

        // Try to extend forward from the chain's tail.
        loop {
            let tail = *chain.last().unwrap();
            let tail_key = key(tail);
            let candidates = endpoint_index.get(&tail_key).cloned().unwrap_or_default();
            let next = candidates.into_iter().find(|wid| !used.contains(wid));
            let Some(next_id) = next else { break };

            let next_pts = &ways[&next_id];
            if next_pts.len() < 2 {
                used.insert(next_id);
                continue;
            }

            used.insert(next_id);
            let next_start_key = key(*next_pts.first().unwrap());
            let next_end_key = key(*next_pts.last().unwrap());

            if next_start_key == tail_key {
                // Append forward (skip duplicate junction point).
                chain.extend_from_slice(&next_pts[1..]);
            } else if next_end_key == tail_key {
                // Append reversed.
                let mut rev = next_pts.clone();
                rev.reverse();
                chain.extend_from_slice(&rev[1..]);
            } else {
                // No clean join (shouldn't happen if index is correct) — just stop.
                break;
            }
        }

        if chain.len() >= 2 {
            chains.push(chain);
        }
    }

    chains
}
