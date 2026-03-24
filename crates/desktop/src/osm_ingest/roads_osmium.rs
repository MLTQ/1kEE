use crate::settings_store;
use std::fs;
use std::path::Path;

use super::OsmFeatureKind;
use super::OsmJob;
use super::db::update_job_note;
use super::job_dispatch::{
    clear_cell_progress, focus_batch_extract_path, focus_cell_bounds, focus_cell_cached,
    focus_cell_extract_path, focus_cells_bounds, focus_cells_for_bounds, mark_focus_cell_cached,
    road_data_gen, run_osmium_extract, set_cell_progress,
};
use super::roads_stream::import_focus_roads_via_stream_scan;

pub(super) fn import_focus_roads_via_osmium(
    db_path: &Path,
    job: &OsmJob,
) -> Result<String, String> {
    let osmium = settings_store::resolve_osmium();
    if std::process::Command::new(&osmium)
        .arg("--version")
        .output()
        .is_err()
    {
        return Err(format!("osmium not found at {}", osmium.display()));
    }

    let extract_dir = db_path
        .parent()
        .ok_or("OSM runtime DB has no parent directory")?
        .join("osm_extracts");
    fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;

    let metadata_connection =
        super::db::open_runtime_db(db_path).map_err(|error| error.to_string())?;
    let source_key = job.source_path.display().to_string();
    let cells = focus_cells_for_bounds(job.bounds);
    let total = cells.len() as u32;
    set_cell_progress(0, total);

    let mut reused_cells = 0usize;
    let mut import_cells = Vec::new();
    let mut missing_extract_cells = Vec::new();
    for &(lat_c, lon_c) in &cells {
        let extract_path = focus_cell_extract_path(&extract_dir, lat_c, lon_c);
        let imported = focus_cell_cached(
            &metadata_connection,
            OsmFeatureKind::Roads,
            &source_key,
            lat_c,
            lon_c,
        )
        .map_err(|error| error.to_string())?;

        if imported {
            reused_cells += 1;
        } else {
            import_cells.push((lat_c, lon_c));
        }

        if !extract_path.exists() {
            missing_extract_cells.push((lat_c, lon_c));
        }
    }

    set_cell_progress(reused_cells as u32, total);
    if import_cells.is_empty() && missing_extract_cells.is_empty() {
        clear_cell_progress();
        return Ok(format!(
            "Focused road osmium import: 0 new cells scanned, {} cached cells reused",
            reused_cells
        ));
    }

    if !missing_extract_cells.is_empty() {
        let pending_bounds = focus_cells_bounds(&missing_extract_cells);
        let batch_path = focus_batch_extract_path(&extract_dir, job.id, OsmFeatureKind::Roads);
        update_job_note(
            db_path,
            job.id,
            &format!(
                "Osmium extracting {} road cells into persistent vector cache…",
                missing_extract_cells.len()
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
                "Splitting batched road extract into {} cached cells…",
                missing_extract_cells.len()
            ),
        )?;

        let split_result = extract_focus_cells_from_batch(
            &osmium,
            db_path,
            job,
            &batch_path,
            &extract_dir,
            &missing_extract_cells,
        );
        let _ = fs::remove_file(&batch_path);
        split_result?;
    }

    let imported_cells = import_cells_from_cache(
        db_path,
        job,
        &extract_dir,
        &source_key,
        &import_cells,
        reused_cells,
        total,
    )?;

    clear_cell_progress();
    Ok(format!(
        "Focused road osmium import: {} cells imported, {} cached cells reused, {} durable extracts available",
        imported_cells,
        reused_cells,
        cells.len()
    ))
}

fn extract_focus_cells_from_batch(
    osmium: &Path,
    db_path: &Path,
    job: &OsmJob,
    batch_path: &Path,
    extract_dir: &Path,
    cells: &[(i32, i32)],
) -> Result<(), String> {
    for (idx, &(lat_c, lon_c)) in cells.iter().enumerate() {
        let extract_path = focus_cell_extract_path(extract_dir, lat_c, lon_c);
        update_job_note(
            db_path,
            job.id,
            &format!(
                "Writing durable road cell {}/{} ({lat_c}°,{lon_c}°)…",
                idx + 1,
                cells.len()
            ),
        )?;
        if let Err(error) = run_osmium_extract(
            osmium,
            batch_path,
            &extract_path,
            focus_cell_bounds(lat_c, lon_c),
        ) {
            let _ = fs::remove_file(&extract_path);
            return Err(error);
        }
    }
    Ok(())
}

fn import_cells_from_cache(
    db_path: &Path,
    job: &OsmJob,
    extract_dir: &Path,
    source_key: &str,
    cells: &[(i32, i32)],
    reused_cells: usize,
    total: u32,
) -> Result<usize, String> {
    let mut imported = 0usize;
    for (idx, &(lat_c, lon_c)) in cells.iter().enumerate() {
        let done = (reused_cells + idx) as u32;
        set_cell_progress(done, total);
        update_job_note(
            db_path,
            job.id,
            &format!(
                "Importing cached road cell {}/{} ({lat_c}°,{lon_c}°)…",
                done + 1,
                total
            ),
        )?;

        let extract_path = focus_cell_extract_path(extract_dir, lat_c, lon_c);
        if !extract_path.exists() {
            return Err(format!(
                "Missing durable road cell extract for {lat_c}°, {lon_c}° at {}",
                extract_path.display()
            ));
        }

        let mut scan_job = job.clone();
        scan_job.source_path = extract_path;
        scan_job.bounds = focus_cell_bounds(lat_c, lon_c);
        import_focus_roads_via_stream_scan(db_path, &scan_job)?;
        mark_focus_cell_cached(db_path, OsmFeatureKind::Roads, source_key, lat_c, lon_c)?;
        imported += 1;
        road_data_gen().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::app::request_repaint();
        set_cell_progress((reused_cells + imported) as u32, total);
    }
    Ok(imported)
}
