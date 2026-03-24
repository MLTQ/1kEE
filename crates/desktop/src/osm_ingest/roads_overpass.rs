use crate::model::GeoPoint;
use std::path::Path;

use super::db::{open_runtime_db, update_job_note};
use super::roads_global::RoadTileWriter;
use super::roads_vector_cache::write_roads_to_vector_cells;
use super::util::canonical_road_class;
use super::{OVERPASS_ENDPOINT, OsmJob, PROGRESS_FLUSH_INTERVAL, RoadPolyline};

pub(super) fn import_focus_roads_via_overpass(
    db_path: &Path,
    job: &OsmJob,
) -> Result<String, String> {
    update_job_note(db_path, job.id, "Querying Overpass API for road geometry…")?;

    let b = job.bounds;
    // `out geom` returns node coordinates inline — no separate node lookup needed.
    let query = format!(
        "[out:json][timeout:30];\
         way[\"highway\"~\"^(motorway|motorway_link|trunk|trunk_link|\
         primary|primary_link|secondary|secondary_link|\
         tertiary|tertiary_link|residential|living_street|unclassified|service)$\"]\
         ({min_lat},{min_lon},{max_lat},{max_lon});\
         out geom;",
        min_lat = b.min_lat,
        min_lon = b.min_lon,
        max_lat = b.max_lat,
        max_lon = b.max_lon,
    );

    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(40))
        .user_agent("1kEE/0.1 (tactical globe; overpass road fetch)")
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?
        .post(OVERPASS_ENDPOINT)
        .body(query)
        .send()
        .map_err(|e| format!("Overpass request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("Overpass returned HTTP {}", response.status()));
    }

    let text = response
        .text()
        .map_err(|e| format!("Reading Overpass response: {e}"))?;
    update_job_note(
        db_path,
        job.id,
        "Parsing road geometry from Overpass response…",
    )?;

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Overpass JSON parse error: {e}"))?;

    let elements = json
        .get("elements")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Overpass response missing 'elements' array".to_owned())?;

    let connection = open_runtime_db(db_path).map_err(|e| e.to_string())?;
    connection
        .execute_batch("BEGIN IMMEDIATE;")
        .map_err(|e| e.to_string())?;
    let mut writer = RoadTileWriter::new(connection);
    let mut cached_roads = Vec::new();

    let mut imported = 0usize;
    let mut skipped = 0usize;

    for element in elements {
        // Only process ways (Overpass can also return nodes/relations).
        if element.get("type").and_then(|v| v.as_str()) != Some("way") {
            continue;
        }

        let way_id = element
            .get("id")
            .and_then(|v| v.as_i64())
            .unwrap_or_default();

        let tags = element.get("tags");
        let highway_val = tags
            .and_then(|t| t.get("highway"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(road_class) = canonical_road_class(highway_val) else {
            skipped += 1;
            continue;
        };

        let road_name = tags.and_then(|t| t.get("name")).and_then(|v| v.as_str());

        // `out geom` puts node coordinates directly in a "geometry" array.
        let geometry = element.get("geometry").and_then(|v| v.as_array());
        let Some(geometry) = geometry else {
            skipped += 1;
            continue;
        };

        let points: Vec<GeoPoint> = geometry
            .iter()
            .filter_map(|node| {
                let lat = node.get("lat")?.as_f64()? as f32;
                let lon = node.get("lon")?.as_f64()? as f32;
                Some(GeoPoint { lat, lon })
            })
            .collect();

        if points.len() < 2 {
            skipped += 1;
            continue;
        }

        cached_roads.push(RoadPolyline {
            way_id,
            road_class: road_class.to_owned(),
            name: road_name.map(ToOwned::to_owned),
            points: points.clone(),
        });
        writer
            .insert_road(way_id, road_class, road_name, &points)
            .map_err(|e| format!("DB insert error: {e}"))?;
        imported += 1;

        if imported % PROGRESS_FLUSH_INTERVAL == 0 {
            let _ = writer.flush_progress();
            let _ = update_job_note(
                db_path,
                job.id,
                &format!("Importing Overpass roads… {imported} written"),
            );
        }
    }

    writer.finish().map_err(|e| e.to_string())?;
    update_job_note(
        db_path,
        job.id,
        "Writing focused road vector cache from Overpass response…",
    )?;
    let cached_cells = write_roads_to_vector_cells(db_path, job.bounds, &cached_roads)?;
    crate::app::request_repaint();

    Ok(format!(
        "Overpass import complete: {imported} roads written, {skipped} skipped, {cached_cells} vector cells cached"
    ))
}
