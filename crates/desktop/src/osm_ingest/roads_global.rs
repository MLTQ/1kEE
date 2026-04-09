use crate::model::GeoPoint;
use osmpbf::{Element, ElementReader};
use rusqlite::Connection;
use rusqlite::params;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::db::{open_runtime_db, update_job_note};
use super::inventory::supports_locations_on_ways_for_path;
use super::util::{
    bounds_intersect, canonical_road_class, encode_linestring_wkb, lat_lon_to_tile,
    polyline_bounds, unix_timestamp,
};
use super::{GeoBounds, OsmJob, PROGRESS_FLUSH_INTERVAL, ROAD_TILE_ZOOMS};

/// Global planet stream scan — imports all road ways from a planet.osm.pbf.
pub(super) fn import_planet_roads(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    if !supports_locations_on_ways_for_path(&job.source_path)? {
        return Err(
            "Planet source does not advertise LocationsOnWays; pure-Rust global road bootstrap is not available on this file yet.".to_owned(),
        );
    }

    let connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    connection
        .execute_batch("BEGIN IMMEDIATE; DELETE FROM road_tiles; DELETE FROM road_tile_manifest;")
        .map_err(|error| error.to_string())?;

    let mut writer = RoadTileWriter::new(connection);
    let reader = ElementReader::from_path(&job.source_path).map_err(|error| {
        format!(
            "Failed to open OSM planet source {}: {error}",
            job.source_path.display()
        )
    })?;

    let mut processed_ways = 0usize;
    let mut import_error: Option<String> = None;
    let mut seen_way_ids = HashSet::new();

    let (tx, rx) = crossbeam_channel::bounded(10000);
    let job_bounds = job.bounds;
    
    let reader_handle = std::thread::spawn(move || {
        let result = reader.par_map_reduce(
            |element| {
                let Element::Way(way) = element else { return };
                
                let mut highway_class = None;
                let mut road_name = None;
                for (key, value) in way.tags() {
                    if key == "highway" {
                        highway_class = canonical_road_class(value);
                    } else if key == "name" && road_name.is_none() {
                        road_name = Some(value.to_owned());
                    }
                }

                let Some(road_class) = highway_class else { return };

                let points: Vec<_> = way
                    .node_locations()
                    .map(|location| GeoPoint {
                        lat: location.lat() as f32,
                        lon: location.lon() as f32,
                    })
                    .collect();

                if points.len() < 2 { return }
                
                let bounds = polyline_bounds(&points);
                if !bounds_intersect(bounds, job_bounds) { return }

                // Send matching roads to writer thread. Break/ignore if channel closes.
                let _ = tx.send((way.id(), road_class, road_name, points));
            },
            || (),
            |_, _| ()
        );
        result.map_err(|e| e.to_string())
    });

    while let Ok((way_id, road_class, road_name, points)) = rx.recv() {
        if !seen_way_ids.insert(way_id) {
            continue;
        }

        processed_ways += 1;
        if let Err(error) = writer.insert_road(way_id, road_class, road_name.as_deref(), &points) {
            import_error = Some(error);
            break;
        }

        if processed_ways % PROGRESS_FLUSH_INTERVAL == 0 {
            let _ = writer.flush_progress();
            let _ = update_job_note(
                db_path,
                job.id,
                &format!(
                    "Scanned {} road ways · wrote {} tile features",
                    processed_ways, writer.inserted_features
                ),
            );
        }
    }
    
    drop(rx);
    
    let reader_result = reader_handle.join().unwrap_or_else(|_| Err("OSM parser thread panicked".to_owned()));
    if let Err(error) = reader_result {
        if import_error.is_none() {
            import_error = Some(error);
        }
    }

    if let Some(error) = import_error {
        let _ = writer.rollback();
        return Err(error);
    }

    let inserted_features = writer.inserted_features;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(format!(
        "Imported {} road ways into {} tile features across {} zoom levels",
        processed_ways,
        inserted_features,
        ROAD_TILE_ZOOMS.len()
    ))
}

// ── RoadTileWriter ─────────────────────────────────────────────────────────

pub(super) struct RoadTileWriter {
    pub(super) connection: Connection,
    pub(super) manifest_counts: HashMap<(u8, u32, u32), usize>,
    pub(super) inserted_features: usize,
}

impl RoadTileWriter {
    pub(super) fn new(connection: Connection) -> Self {
        Self {
            connection,
            manifest_counts: HashMap::new(),
            inserted_features: 0,
        }
    }

    pub(super) fn insert_road(
        &mut self,
        way_id: i64,
        road_class: &'static str,
        road_name: Option<&str>,
        points: &[GeoPoint],
    ) -> Result<(), String> {
        let bounds = polyline_bounds(points);
        let wkb = encode_linestring_wkb(points);

        for &zoom in ROAD_TILE_ZOOMS {
            let (min_x, min_y) = lat_lon_to_tile(bounds.max_lat, bounds.min_lon, zoom);
            let (max_x, max_y) = lat_lon_to_tile(bounds.min_lat, bounds.max_lon, zoom);
            for tile_x in min_x.min(max_x)..=min_x.max(max_x) {
                for tile_y in min_y.min(max_y)..=min_y.max(max_y) {
                    self.connection
                        .execute(
                            "INSERT OR REPLACE INTO road_tiles (
                                zoom, tile_x, tile_y, way_id, class, name, geom_wkb,
                                min_lat, max_lat, min_lon, max_lon
                             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                            params![
                                i64::from(zoom),
                                i64::from(tile_x),
                                i64::from(tile_y),
                                way_id,
                                road_class,
                                road_name.unwrap_or(""),
                                &wkb,
                                bounds.min_lat,
                                bounds.max_lat,
                                bounds.min_lon,
                                bounds.max_lon,
                            ],
                        )
                        .map_err(|error| error.to_string())?;
                    *self
                        .manifest_counts
                        .entry((zoom, tile_x, tile_y))
                        .or_insert(0) += 1;
                    self.inserted_features += 1;
                }
            }
        }

        Ok(())
    }

    pub(super) fn flush_progress(&self) -> Result<(), String> {
        self.connection
            .execute_batch("COMMIT; BEGIN IMMEDIATE;")
            .map_err(|error| error.to_string())
    }

    pub(super) fn finish(mut self) -> rusqlite::Result<()> {
        let built_at = unix_timestamp();
        for ((zoom, tile_x, tile_y), feature_count) in self.manifest_counts.drain() {
            let live_count: i64 = self.connection.query_row(
                "SELECT COUNT(*) FROM road_tiles
                 WHERE zoom = ?1 AND tile_x = ?2 AND tile_y = ?3",
                params![i64::from(zoom), i64::from(tile_x), i64::from(tile_y)],
                |row| row.get(0),
            )?;
            self.connection.execute(
                "INSERT OR REPLACE INTO road_tile_manifest (
                    zoom, tile_x, tile_y, feature_count, built_at_unix
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    i64::from(zoom),
                    i64::from(tile_x),
                    i64::from(tile_y),
                    live_count.max(feature_count as i64),
                    built_at,
                ],
            )?;
        }

        self.connection.execute_batch("COMMIT;")
    }

    pub(super) fn rollback(&self) -> Result<(), String> {
        self.connection
            .execute_batch("ROLLBACK;")
            .map_err(|error| error.to_string())
    }
}

/// Load road polylines from the runtime DB for the given tile bounds.
pub(super) fn load_roads_for_bounds_inner(
    db_path: &Path,
    bounds: GeoBounds,
    tile_zoom: u8,
    layer_kind: super::RoadLayerKind,
) -> Vec<super::RoadPolyline> {
    let Ok(connection) = open_runtime_db(db_path) else {
        return Vec::new();
    };
    let (min_x, min_y) = lat_lon_to_tile(bounds.max_lat, bounds.min_lon, tile_zoom);
    let (max_x, max_y) = lat_lon_to_tile(bounds.min_lat, bounds.max_lon, tile_zoom);
    let Ok(mut statement) = connection.prepare(
        "SELECT way_id, class, name, geom_wkb
         FROM road_tiles
         WHERE zoom = ?1
           AND tile_x BETWEEN ?2 AND ?3
           AND tile_y BETWEEN ?4 AND ?5",
    ) else {
        return Vec::new();
    };

    let rows = match statement.query_map(
        params![
            i64::from(tile_zoom),
            i64::from(min_x.min(max_x)),
            i64::from(min_x.max(max_x)),
            i64::from(min_y.min(max_y)),
            i64::from(min_y.max(max_y)),
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        },
    ) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };

    use super::util::{decode_linestring_wkb, road_class_matches};
    use std::collections::HashSet;
    let mut seen_way_ids = HashSet::new();
    let mut roads = Vec::new();
    for row in rows.filter_map(Result::ok) {
        let (way_id, road_class, name, geom_wkb) = row;
        if !road_class_matches(&road_class, layer_kind) || !seen_way_ids.insert(way_id) {
            continue;
        }
        let Some(points) = decode_linestring_wkb(&geom_wkb) else {
            continue;
        };
        let road_bounds = polyline_bounds(&points);
        if !bounds_intersect(road_bounds, bounds) {
            continue;
        }
        roads.push(super::RoadPolyline {
            way_id,
            road_class,
            name: if name.is_empty() { None } else { Some(name) },
            points,
        });
    }

    roads
}
