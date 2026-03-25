use crate::settings_store;
use std::fs;
use std::path::Path;

use super::OsmFeatureKind;
use super::OsmJob;
use super::db::update_job_note;
use super::job_dispatch::{
    clear_cell_progress, focus_batch_extract_path, focus_cell_bounds, focus_cell_extract_path,
    focus_cells_bounds, focus_cells_for_bounds, road_data_gen, run_osmium_extract,
    set_cell_progress,
};
use super::roads_vector_cache::{
    ensure_cell_geojson_from_extract, vector_cache_dir, vector_cell_path,
};

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
    let vector_dir = vector_cache_dir(db_path)?;
    let cells = focus_cells_for_bounds(job.bounds);
    let total = cells.len() as u32;
    set_cell_progress(0, total);

    let mut ready_cells = 0usize;
    let mut missing_extract_cells = Vec::new();
    let mut vector_build_cells = Vec::new();
    for &(lat_c, lon_c) in &cells {
        let extract_path = focus_cell_extract_path(&extract_dir, lat_c, lon_c);
        let vector_path = vector_cell_path(&vector_dir, lat_c, lon_c);
        if vector_path.exists() {
            ready_cells += 1;
        } else if !extract_path.exists() {
            missing_extract_cells.push((lat_c, lon_c));
        } else {
            vector_build_cells.push((lat_c, lon_c));
        }
    }

    set_cell_progress(ready_cells as u32, total);
    if vector_build_cells.is_empty() && missing_extract_cells.is_empty() {
        clear_cell_progress();
        return Ok(format!(
            "Focused road osmium import: 0 new cells scanned, {} direct vector cells reused",
            ready_cells
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

        vector_build_cells.extend(missing_extract_cells.iter().copied());
    }

    let built_vectors = ensure_vector_cells(
        db_path,
        job,
        &extract_dir,
        &vector_dir,
        &vector_build_cells,
        ready_cells,
        total,
    )?;

    clear_cell_progress();
    Ok(format!(
        "Focused road osmium import: {} vector cells built, {} direct vector cells reused, {} durable extracts available",
        built_vectors,
        ready_cells,
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

fn ensure_vector_cells(
    db_path: &Path,
    job: &OsmJob,
    extract_dir: &Path,
    vector_dir: &Path,
    cells: &[(i32, i32)],
    ready_cells: usize,
    total: u32,
) -> Result<usize, String> {
    let mut built = 0usize;
    for (idx, &(lat_c, lon_c)) in cells.iter().enumerate() {
        let done = (ready_cells + idx) as u32;
        set_cell_progress(done, total);
        update_job_note(
            db_path,
            job.id,
            &format!(
                "Building direct road vectors {}/{} ({lat_c}°,{lon_c}°)…",
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

        let vector_path = vector_cell_path(vector_dir, lat_c, lon_c);
        let feature_count = ensure_cell_geojson_from_extract(
            &extract_path,
            &vector_path,
            focus_cell_bounds(lat_c, lon_c),
        )?;
        update_job_note(
            db_path,
            job.id,
            &format!(
                "Cached direct road vectors for cell {}/{} ({lat_c}°,{lon_c}°) — {} features",
                done + 1,
                total,
                feature_count
            ),
        )?;

        built += 1;
        road_data_gen().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::app::request_repaint();
        set_cell_progress((ready_cells + built) as u32, total);
    }
    Ok(built)
}
