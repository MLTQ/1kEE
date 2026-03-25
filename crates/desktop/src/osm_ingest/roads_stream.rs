use crate::model::GeoPoint;
use osmpbf::{Element, ElementReader};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::db::{open_runtime_db, update_job_note};
use super::roads_global::RoadTileWriter;
use super::util::{
    bounds_intersect, canonical_road_class, expand_bounds, point_in_bounds, polyline_bounds,
};
use super::{
    FOCUS_NODE_MARGIN_DEGREES, FOCUS_SCAN_PROGRESS_INTERVAL, GeoBounds, OsmJob,
    PROGRESS_FLUSH_INTERVAL,
};

pub(super) fn import_focus_roads_via_stream_scan(
    db_path: &Path,
    job: &OsmJob,
) -> Result<String, String> {
    update_job_note(
        db_path,
        job.id,
        "Scanning focused road geometry from the planet source...",
    )?;

    let expanded_bounds = expand_bounds(job.bounds, FOCUS_NODE_MARGIN_DEGREES);
    let connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    connection
        .execute_batch("BEGIN IMMEDIATE;")
        .map_err(|error| error.to_string())?;
    let mut writer = RoadTileWriter::new(connection);

    let reader = ElementReader::from_path(&job.source_path).map_err(|error| {
        format!(
            "Failed to open OSM planet source {}: {error}",
            job.source_path.display()
        )
    })?;

    let mut candidate_nodes: HashMap<i64, GeoPoint> = HashMap::new();
    let mut seen_way_ids = HashSet::new();
    let mut scanned_nodes = 0usize;
    let mut scanned_ways = 0usize;
    let mut imported_roads = 0usize;
    let mut import_error: Option<String> = None;

    reader
        .for_each(|element| {
            if import_error.is_some() {
                return;
            }

            match element {
                Element::Node(node) => {
                    scanned_nodes += 1;
                    let point = GeoPoint {
                        lat: node.lat() as f32,
                        lon: node.lon() as f32,
                    };
                    if point_in_bounds(point, expanded_bounds) {
                        candidate_nodes.insert(node.id(), point);
                    }
                }
                Element::DenseNode(node) => {
                    scanned_nodes += 1;
                    let point = GeoPoint {
                        lat: node.lat() as f32,
                        lon: node.lon() as f32,
                    };
                    if point_in_bounds(point, expanded_bounds) {
                        candidate_nodes.insert(node.id(), point);
                    }
                }
                Element::Way(way) => {
                    scanned_ways += 1;

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

                    let bounds = polyline_bounds(&points);
                    if !bounds_intersect(bounds, job.bounds) {
                        return;
                    }

                    imported_roads += 1;
                    if let Err(error) =
                        writer.insert_road(way.id(), road_class, road_name.as_deref(), &points)
                    {
                        import_error = Some(error);
                        return;
                    }

                    if imported_roads % PROGRESS_FLUSH_INTERVAL == 0 {
                        let _ = writer.flush_progress();
                    }
                }
                Element::Relation(_) => {}
            }

            let processed = scanned_nodes.saturating_add(scanned_ways);
            if processed > 0 && processed % FOCUS_SCAN_PROGRESS_INTERVAL == 0 {
                let _ = update_job_note(
                    db_path,
                    job.id,
                    &format!(
                        "Scanned {} nodes, {} ways · kept {} candidate nodes · imported {} roads",
                        scanned_nodes,
                        scanned_ways,
                        candidate_nodes.len(),
                        imported_roads
                    ),
                );
            }
        })
        .map_err(|error| error.to_string())?;

    if let Some(error) = import_error {
        let _ = writer.rollback();
        return Err(error);
    }

    let inserted_features = writer.inserted_features;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(format!(
        "Imported {} focused roads from {} kept nodes into {} tile features",
        imported_roads,
        candidate_nodes.len(),
        inserted_features
    ))
}
