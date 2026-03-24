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

pub(super) fn import_focus_roads_via_osmium(db_path: &Path, job: &OsmJob) -> Result<String, String> {
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

    let metadata_connection = super::db::open_runtime_db(db_path).map_err(|error| error.to_string())?;
    let source_key = job.source_path.display().to_string();
    let cells = focus_cells_for_bounds(job.bounds);
    let total = cells.len() as u32;
    set_cell_progress(0, total);

    let mut reused_cells = 0usize;
    let mut pending_cells = Vec::new();
    for &(lat_c, lon_c) in &cells {
        if focus_cell_cached(
            &metadata_connection,
            OsmFeatureKind::Roads,
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
            "Focused road osmium import: 0 new cells scanned, {} cached cells reused",
            reused_cells
        ));
    }

    let imported_cells = if pending_cells
        .iter()
        .any(|&(lat_c, lon_c)| !focus_cell_extract_path(&extract_dir, lat_c, lon_c).exists())
    {
        let pending_bounds = focus_cells_bounds(&pending_cells);
        let batch_path = focus_batch_extract_path(&extract_dir, job.id, OsmFeatureKind::Roads);
        update_job_note(
            db_path,
            job.id,
            &format!(
                "Osmium extract {} road cells as one batch — one-time, ~2-5 min…",
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
                "Scanning batched road extract for {} focused cells…",
                pending_cells.len()
            ),
        )?;
        let mut scan_job = job.clone();
        scan_job.source_path = batch_path.clone();
        scan_job.bounds = pending_bounds;
        let result = import_focus_roads_via_stream_scan(db_path, &scan_job);
        let _ = fs::remove_file(&batch_path);
        result?;

        for (idx, &(lat_c, lon_c)) in pending_cells.iter().enumerate() {
            mark_focus_cell_cached(db_path, OsmFeatureKind::Roads, &source_key, lat_c, lon_c)?;
            set_cell_progress((reused_cells + idx + 1) as u32, total);
        }
        road_data_gen().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                    "Scanning cached road cell {}/{} ({lat_c}°,{lon_c}°)…",
                    done + 1,
                    total
                ),
            )?;
            let mut scan_job = job.clone();
            scan_job.source_path = focus_cell_extract_path(&extract_dir, lat_c, lon_c);
            scan_job.bounds = focus_cell_bounds(lat_c, lon_c);
            import_focus_roads_via_stream_scan(db_path, &scan_job)?;
            mark_focus_cell_cached(db_path, OsmFeatureKind::Roads, &source_key, lat_c, lon_c)?;
            imported += 1;
            road_data_gen().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::app::request_repaint();
            set_cell_progress((reused_cells + imported) as u32, total);
        }
        imported
    };

    clear_cell_progress();
    Ok(format!(
        "Focused road osmium import: {} new cells scanned, {} cached cells reused",
        imported_cells, reused_cells
    ))
}
