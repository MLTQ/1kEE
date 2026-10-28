use crate::args::BboxCommand;
use crate::geojson::write_admin_level_file;
use crate::node_store::NodeStore;
use crate::roads::{RoadBuildProgress, open_planet_at};
use crate::util::{GeoBounds, GeoPoint};
#[path = "admin_geometry.rs"]
mod geometry;
pub use geometry::AdminRegion;
use osmpbf::{BlobDecode, RelMemberType};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

const RELATION_BATCH: usize = 500;
const WAY_BATCH: usize = 1000;

/// Top-level entry point. Runs Pass R and Pass A if not yet complete, then stitches
/// rings and writes per-level binary files.
///
/// Returns the total number of boundary rings written.
pub fn load_or_build_admin_boundaries(
    command: &BboxCommand,
    _bounds: GeoBounds,
    node_store: &mut NodeStore,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<usize, String> {
    node_store.prepare_admin_source(&command.planet_path)?;
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
    stitch_and_write(
        &command.cache_dir,
        node_store,
        false,
        &mut |ids| {
            // Missing nodes break geometry instead of joining across gaps.
            node_store.optional_points_for_refs(ids)
        },
        progress,
    )
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
    let mut checkpoint = start;

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

            // OSM levels vary by country; retain intermediate and local divisions too.
            let mut is_boundary = false;
            let mut admin_level: Option<u8> = None;
            let mut name: Option<String> = None;

            for (key, value) in rel.tags() {
                match key {
                    "boundary" if value == "administrative" => is_boundary = true,
                    "admin_level" => {
                        admin_level = value.parse::<u8>().ok().filter(|n| (2..=11).contains(n));
                    }
                    "name" if name.is_none() => name = Some(value.to_owned()),
                    _ => {}
                }
            }

            if !is_boundary {
                continue;
            }
            let Some(level) = admin_level else { continue };

            // Nested geometry relations need recursive resolution. Do not pretend
            // a partial relation is a complete polygon (subarea/label members are metadata).
            if rel.members().any(|m| {
                m.member_type == RelMemberType::Relation
                    && matches!(m.role().unwrap_or(""), "" | "outer" | "inner")
            }) {
                continue;
            }
            // Keep inner members: even-odd filling must preserve enclaves/holes.
            let way_ids: Vec<i64> = rel
                .members()
                .filter_map(|m| {
                    if m.member_type != RelMemberType::Way {
                        return None;
                    }
                    let role = m.role().unwrap_or("");
                    if !matches!(role, "" | "outer" | "inner") {
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
        }
        let offset = pos.load(Ordering::Relaxed);
        if batch.len() >= RELATION_BATCH || offset.saturating_sub(checkpoint) >= 256 * 1024 * 1024 {
            node_store.save_admin_relations_batch(&batch)?;
            batch.clear();
            node_store.save_scan_offset("relation_scan", offset)?;
            checkpoint = offset;
            progress(RoadBuildProgress {
                stage: "Scanning Relations".into(),
                fraction: 0.89,
                message: format!(
                    "{total_relations} records; {:.1} GB scanned",
                    offset as f64 / 1e9
                ),
            });
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
    let mut checkpoint = start;

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
        }
        let offset = pos.load(Ordering::Relaxed);
        if batch.len() >= WAY_BATCH || offset.saturating_sub(checkpoint) >= 256 * 1024 * 1024 {
            node_store.save_admin_way_nodes(&batch)?;
            batch.clear();
            node_store.save_scan_offset("admin_way_scan", offset)?;
            checkpoint = offset;
            progress(RoadBuildProgress {
                stage: "Scanning Admin Ways".into(),
                fraction: 0.89,
                message: format!(
                    "{ways_found} records; {:.1} GB scanned",
                    offset as f64 / 1e9
                ),
            });
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

/// Reuse either planet node backend; administrative passes have their own checkpoints.
pub fn build_planet(
    command: &crate::args::PlanetAllCommand,
    lookup: &crate::planet_lookup::Lookup,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<usize, String> {
    let scratch = if command.node_storage == crate::args::NodeStorage::IndexedPbf {
        command.tmp_dir.join("indexed-pbf-v1")
    } else {
        command.tmp_dir.clone()
    };
    let mut store = NodeStore::open(&scratch.join("admin-v2.sqlite"))?;
    store.prepare_admin_source(&command.planet_path)?;
    if !store.is_relation_scan_complete()? {
        collect_admin_relations(&command.planet_path, &mut store, progress)?;
    }
    if !store.is_admin_way_scan_complete()? {
        collect_admin_way_nodes(&command.planet_path, &mut store, progress)?;
    }
    let mut session = lookup.session();
    lookup.validate_source()?;
    let count = stitch_and_write(
        &command.out_dir,
        &store,
        true,
        &mut |ids| {
            session.lookup_many(ids).map(|points| {
                points
                    .into_iter()
                    .map(|p| p.map(|(lat, lon)| GeoPoint { lat, lon }))
                    .collect()
            })
        },
        progress,
    )?;
    lookup.validate_source()?;
    Ok(count)
}

fn stitch_and_write(
    cache_dir: &std::path::Path,
    node_store: &NodeStore,
    replace: bool,
    resolve: &mut geometry::CoordinateResolver<'_>,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<usize, String> {
    let mut by_level: HashMap<u8, Vec<AdminRegion>> = HashMap::new();
    for (relation_id, name, level, way_ids) in node_store.get_admin_relation_ways()? {
        let ways = node_store.admin_way_refs(&way_ids)?;
        let mut region = geometry::assemble(relation_id, name, &ways, resolve)?;
        region.complete &= ways.len() == way_ids.len();
        if !region.rings.is_empty() {
            by_level.entry(level).or_default().push(region);
        }
    }
    let mut total = 0;
    for level in 2..=11 {
        total += write_admin_level_file(
            cache_dir,
            level,
            by_level.get(&level).map_or(&[], Vec::as_slice),
            replace,
        )?;
    }
    progress(RoadBuildProgress {
        stage: "Writing Admin Boundaries".into(),
        fraction: 0.98,
        message: format!(
            "Wrote {total} admin boundary rings across {} levels",
            by_level.len()
        ),
    });
    Ok(total)
}
