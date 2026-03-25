use crate::model::GeoPoint;
use osmpbf::{Element, ElementReader};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use super::db::{open_runtime_db, update_job_note};
use super::job_dispatch::{
    clear_cell_progress, focus_batch_extract_path, focus_cell_bounds, focus_cell_cached,
    focus_cell_extract_path, focus_cells_bounds, focus_cells_for_bounds, mark_focus_cell_cached,
    run_osmium_extract, set_cell_progress, water_data_gen,
};
use super::util::{
    bounds_intersect, canonical_water_class, encode_linestring_wkb, expand_bounds, lat_lon_to_tile,
    point_in_bounds, polyline_bounds, unix_timestamp,
};
use super::{
    FOCUS_NODE_MARGIN_DEGREES, GeoBounds, OVERPASS_ENDPOINT, OsmFeatureKind, OsmJob,
    PROGRESS_FLUSH_INTERVAL, ROAD_TILE_ZOOMS, WaterPolyline,
};

/// Top-level water import dispatcher (called from tick()).
pub(super) fn import_planet_water(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    if job.source_path == std::path::Path::new(super::OVERPASS_SOURCE) {
        return import_focus_water_via_overpass(db_path, job);
    }
    if crate::settings_store::prefer_overpass() {
        return import_focus_water_via_overpass(db_path, job);
    }
    import_focus_water_via_osmium(db_path, job).or_else(|e| {
        let _ = update_job_note(
            db_path,
            job.id,
            &format!("osmium unavailable ({e}); falling back to Overpass…"),
        );
        import_focus_water_via_overpass(db_path, job)
    })
}

fn import_focus_water_via_osmium(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    let osmium = crate::settings_store::resolve_osmium();
    if std::process::Command::new(&osmium)
        .arg("--version")
        .output()
        .is_err()
    {
        return Err(format!("osmium not found at {}", osmium.display()));
    }

    let extract_dir = db_path
        .parent()
        .ok_or("no parent dir")?
        .join("osm_extracts");
    fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;

    let metadata_connection = open_runtime_db(db_path).map_err(|error| error.to_string())?;
    let source_key = job.source_path.display().to_string();
    let cells = focus_cells_for_bounds(job.bounds);
    let total = cells.len() as u32;
    set_cell_progress(0, total);

    let mut reused_cells = 0usize;
    let mut pending_cells = Vec::new();
    for &(lat_c, lon_c) in &cells {
        if focus_cell_cached(
            &metadata_connection,
            OsmFeatureKind::Water,
            &source_key,
            lat_c,
            lon_c,
        )
        .map_err(|error| error.to_string())?
        {
            reused_cells += 1;
        } else {
            pending_cells.push((lat_c, lon_c));
        }
    }

    set_cell_progress(reused_cells as u32, total);
    if pending_cells.is_empty() {
        clear_cell_progress();
        return Ok(format!(
            "Focused water osmium import: 0 new cells scanned, {} cached cells reused",
            reused_cells
        ));
    }

    let imported_cells = if pending_cells
        .iter()
        .any(|&(lat_c, lon_c)| !focus_cell_extract_path(&extract_dir, lat_c, lon_c).exists())
    {
        let pending_bounds = focus_cells_bounds(&pending_cells);
        let batch_path = focus_batch_extract_path(&extract_dir, job.id, OsmFeatureKind::Water);
        update_job_note(
            db_path,
            job.id,
            &format!(
                "Osmium extract {} water cells as one batch — one-time, ~2-5 min…",
                pending_cells.len()
            ),
        )?;
        if let Err(error) =
            run_osmium_extract(&osmium, &job.source_path, &batch_path, pending_bounds)
        {
            let _ = fs::remove_file(&batch_path);
            clear_cell_progress();
            return Err(error);
        }

        update_job_note(
            db_path,
            job.id,
            &format!(
                "Scanning batched water extract for {} focused cells…",
                pending_cells.len()
            ),
        )?;
        let mut scan_job = job.clone();
        scan_job.source_path = batch_path.clone();
        scan_job.bounds = pending_bounds;
        let result = import_focus_water_via_stream_scan(db_path, &scan_job);
        let _ = fs::remove_file(&batch_path);
        result?;

        for (idx, &(lat_c, lon_c)) in pending_cells.iter().enumerate() {
            mark_focus_cell_cached(db_path, OsmFeatureKind::Water, &source_key, lat_c, lon_c)?;
            set_cell_progress((reused_cells + idx + 1) as u32, total);
        }
        water_data_gen().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::app::request_repaint();
        pending_cells.len()
    } else {
        let mut imported = 0usize;
        for (idx, &(lat_c, lon_c)) in pending_cells.iter().enumerate() {
            let done = (reused_cells + idx) as u32;
            set_cell_progress(done, total);
            update_job_note(
                db_path,
                job.id,
                &format!(
                    "Scanning cached water cell {}/{} ({lat_c}°,{lon_c}°)…",
                    done + 1,
                    total
                ),
            )?;
            let mut scan_job = job.clone();
            scan_job.source_path = focus_cell_extract_path(&extract_dir, lat_c, lon_c);
            scan_job.bounds = focus_cell_bounds(lat_c, lon_c);
            import_focus_water_via_stream_scan(db_path, &scan_job)?;
            mark_focus_cell_cached(db_path, OsmFeatureKind::Water, &source_key, lat_c, lon_c)?;
            imported += 1;
            water_data_gen().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::app::request_repaint();
            set_cell_progress((reused_cells + imported) as u32, total);
        }
        imported
    };

    clear_cell_progress();
    Ok(format!(
        "Focused water osmium import: {} new cells scanned, {} cached cells reused",
        imported_cells, reused_cells
    ))
}

fn import_focus_water_via_stream_scan(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    update_job_note(
        db_path,
        job.id,
        "Scanning water features from planet source…",
    )?;

    let expanded = expand_bounds(job.bounds, FOCUS_NODE_MARGIN_DEGREES);
    let conn = open_runtime_db(db_path).map_err(|e| e.to_string())?;
    conn.execute_batch("BEGIN IMMEDIATE;")
        .map_err(|e| e.to_string())?;
    let mut writer = WaterTileWriter::new(conn);

    let reader = ElementReader::from_path(&job.source_path)
        .map_err(|e| format!("Failed to open {}: {e}", job.source_path.display()))?;

    let mut candidate_nodes: HashMap<i64, GeoPoint> = HashMap::new();
    let mut seen_way_ids = HashSet::new();
    let mut imported = 0usize;
    let mut import_error: Option<String> = None;

    reader
        .for_each(|element| {
            if import_error.is_some() {
                return;
            }
            match element {
                Element::Node(n) => {
                    let pt = GeoPoint {
                        lat: n.lat() as f32,
                        lon: n.lon() as f32,
                    };
                    if point_in_bounds(pt, expanded) {
                        candidate_nodes.insert(n.id(), pt);
                    }
                }
                Element::DenseNode(n) => {
                    let pt = GeoPoint {
                        lat: n.lat() as f32,
                        lon: n.lon() as f32,
                    };
                    if point_in_bounds(pt, expanded) {
                        candidate_nodes.insert(n.id(), pt);
                    }
                }
                Element::Way(way) => {
                    let mut water_class: Option<(&'static str, bool)> = None;
                    let mut feat_name = None;
                    for (k, v) in way.tags() {
                        if water_class.is_none() {
                            water_class = canonical_water_class(k, v);
                        }
                        if k == "name" && feat_name.is_none() {
                            feat_name = Some(v.to_owned());
                        }
                    }
                    let Some((class, is_area)) = water_class else {
                        return;
                    };
                    if !seen_way_ids.insert(way.id()) {
                        return;
                    }

                    let refs: Vec<i64> = way.refs().collect();
                    let closed = refs.first() == refs.last() && refs.len() > 2;
                    let is_area = is_area || closed;

                    let points: Vec<GeoPoint> = refs
                        .iter()
                        .filter_map(|&id| candidate_nodes.get(&id).copied())
                        .collect();
                    if points.len() < 2 {
                        return;
                    }

                    let bounds = polyline_bounds(&points);
                    if !bounds_intersect(bounds, job.bounds) {
                        return;
                    }

                    imported += 1;
                    if let Err(e) =
                        writer.insert_water(way.id(), class, feat_name.as_deref(), is_area, &points)
                    {
                        import_error = Some(e);
                    }
                    if imported % PROGRESS_FLUSH_INTERVAL == 0 {
                        let _ = writer.flush_progress();
                    }
                }
                Element::Relation(_) => {}
            }
        })
        .map_err(|e| e.to_string())?;

    if let Some(e) = import_error {
        let _ = writer.rollback();
        return Err(e);
    }
    let total = writer.inserted_features;
    writer.finish_simple().map_err(|e| e.to_string())?;
    Ok(format!(
        "Water stream scan: {imported} features → {total} tile entries"
    ))
}

fn import_focus_water_via_overpass(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    update_job_note(db_path, job.id, "Querying Overpass API for water features…")?;

    let b = job.bounds;
    let query = format!(
        "[out:json][timeout:60];\
         (\
           way[\"waterway\"~\"^(river|stream|canal|drain|creek|ditch)$\"]\
             ({min_lat},{min_lon},{max_lat},{max_lon});\
           way[\"natural\"=\"water\"]\
             ({min_lat},{min_lon},{max_lat},{max_lon});\
           way[\"landuse\"~\"^(reservoir|basin)$\"]\
             ({min_lat},{min_lon},{max_lat},{max_lon});\
         );\
         out geom;",
        min_lat = b.min_lat,
        min_lon = b.min_lon,
        max_lat = b.max_lat,
        max_lon = b.max_lon,
    );

    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
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
        .map_err(|e| format!("Failed to read Overpass response: {e}"))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| format!("Invalid JSON: {e}"))?;

    let conn = open_runtime_db(db_path).map_err(|e| e.to_string())?;
    conn.execute_batch("BEGIN IMMEDIATE;")
        .map_err(|e| e.to_string())?;
    let mut writer = WaterTileWriter::new(conn);
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut synthetic_id = -1_i64;

    if let Some(elements) = json.get("elements").and_then(|v| v.as_array()) {
        for element in elements {
            let tags = element.get("tags").and_then(|t| t.as_object());
            let (class, is_area_hint) = tags
                .and_then(|t| {
                    for (k, v) in t {
                        if let Some(r) = canonical_water_class(k, v.as_str().unwrap_or("")) {
                            return Some(r);
                        }
                    }
                    None
                })
                .unwrap_or_else(|| {
                    skipped += 1;
                    ("", false)
                });
            if class.is_empty() {
                continue;
            }

            let name = tags
                .and_then(|t| t.get("name"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned);

            let points: Vec<GeoPoint> = element
                .get("geometry")
                .and_then(|g| g.as_array())
                .map(|pts| {
                    pts.iter()
                        .filter_map(|p| {
                            Some(GeoPoint {
                                lat: p.get("lat")?.as_f64()? as f32,
                                lon: p.get("lon")?.as_f64()? as f32,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            if points.len() < 2 {
                skipped += 1;
                continue;
            }

            let closed = points.first().map(|p| p.lat) == points.last().map(|p| p.lat)
                && points.first().map(|p| p.lon) == points.last().map(|p| p.lon)
                && points.len() > 2;
            let is_area = is_area_hint || closed;

            let way_id = element
                .get("id")
                .and_then(|v| v.as_i64())
                .unwrap_or_else(|| {
                    synthetic_id -= 1;
                    synthetic_id
                });

            if let Err(e) = writer.insert_water(way_id, class, name.as_deref(), is_area, &points) {
                let _ = writer.rollback();
                return Err(e);
            }
            imported += 1;

            if imported % PROGRESS_FLUSH_INTERVAL == 0 {
                let _ = writer.flush_progress();
                let _ = update_job_note(
                    db_path,
                    job.id,
                    &format!("Overpass water import… {imported} written"),
                );
            }
        }
    }

    writer.finish_simple().map_err(|e| e.to_string())?;
    crate::app::request_repaint();
    Ok(format!(
        "Overpass water import: {imported} features, {skipped} skipped"
    ))
}

/// Load water features for a viewport tile range from the runtime SQLite DB.
pub fn load_water_for_bounds(
    selected_root: Option<&std::path::Path>,
    bounds: GeoBounds,
    tile_zoom: u8,
) -> Vec<WaterPolyline> {
    use super::db::runtime_db_path;
    use super::util::decode_linestring_wkb;

    let Some(db_path) = runtime_db_path(selected_root) else {
        return Vec::new();
    };
    if !db_path.exists() {
        return Vec::new();
    }
    let Ok(conn) = open_runtime_db(&db_path) else {
        return Vec::new();
    };

    let (x0, y0) = lat_lon_to_tile(bounds.max_lat, bounds.min_lon, tile_zoom);
    let (x1, y1) = lat_lon_to_tile(bounds.min_lat, bounds.max_lon, tile_zoom);
    let Ok(mut stmt) = conn.prepare(
        "SELECT way_id, class, name, is_area, geom_wkb
         FROM water_tiles
         WHERE zoom=?1 AND tile_x BETWEEN ?2 AND ?3 AND tile_y BETWEEN ?4 AND ?5",
    ) else {
        return Vec::new();
    };

    let rows = match stmt.query_map(
        params![
            i64::from(tile_zoom),
            i64::from(x0.min(x1)),
            i64::from(x0.max(x1)),
            i64::from(y0.min(y1)),
            i64::from(y0.max(y1)),
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        },
    ) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for row in rows.filter_map(Result::ok) {
        let (way_id, water_class, name, is_area, wkb) = row;
        if !seen.insert(way_id) {
            continue;
        }
        let Some(points) = decode_linestring_wkb(&wkb) else {
            continue;
        };
        let poly_bounds = polyline_bounds(&points);
        if !bounds_intersect(poly_bounds, bounds) {
            continue;
        }
        out.push(WaterPolyline {
            way_id,
            water_class,
            name: if name.is_empty() { None } else { Some(name) },
            points,
            is_area: is_area != 0,
        });
    }
    out
}

// ── WaterTileWriter ────────────────────────────────────────────────────────

struct WaterTileWriter {
    connection: Connection,
    inserted_features: usize,
}

impl WaterTileWriter {
    fn new(connection: Connection) -> Self {
        Self {
            connection,
            inserted_features: 0,
        }
    }

    fn insert_water(
        &mut self,
        way_id: i64,
        class: &str,
        name: Option<&str>,
        is_area: bool,
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
                            "INSERT OR REPLACE INTO water_tiles (
                            zoom, tile_x, tile_y, way_id, class, name, is_area,
                            geom_wkb, min_lat, max_lat, min_lon, max_lon
                         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                            params![
                                i64::from(zoom),
                                i64::from(tile_x),
                                i64::from(tile_y),
                                way_id,
                                class,
                                name.unwrap_or(""),
                                if is_area { 1i64 } else { 0i64 },
                                &wkb,
                                bounds.min_lat,
                                bounds.max_lat,
                                bounds.min_lon,
                                bounds.max_lon,
                            ],
                        )
                        .map_err(|e| e.to_string())?;
                    self.inserted_features += 1;
                }
            }
        }
        Ok(())
    }

    fn flush_progress(&self) -> Result<(), String> {
        self.connection
            .execute_batch("COMMIT; BEGIN IMMEDIATE;")
            .map_err(|e| e.to_string())
    }

    fn finish_simple(self) -> rusqlite::Result<()> {
        self.connection.execute_batch("COMMIT;")?;
        Ok(())
    }

    fn rollback(&self) -> Result<(), String> {
        self.connection
            .execute_batch("ROLLBACK;")
            .map_err(|e| e.to_string())
    }
}
