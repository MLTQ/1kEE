use crate::settings_store;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use super::OsmJob;
use super::db::update_job_note;
use super::job_dispatch::{clear_cell_progress, road_data_gen, set_cell_progress};
use super::roads_stream::import_focus_roads_via_stream_scan;

pub(super) fn import_focus_roads_via_osmium(db_path: &Path, job: &OsmJob) -> Result<String, String> {
    let osmium = settings_store::resolve_osmium();
    if Command::new(&osmium).arg("--version").output().is_err() {
        return Err(format!("osmium not found at {}", osmium.display()));
    }

    let extract_dir = db_path
        .parent()
        .ok_or("OSM runtime DB has no parent directory")?
        .join("osm_extracts");
    fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;

    // Enumerate every 1°×1° cell that overlaps the job's bounding box.
    let min_lat_c = job.bounds.min_lat.floor() as i32;
    let max_lat_c = job.bounds.max_lat.floor() as i32;
    let min_lon_c = job.bounds.min_lon.floor() as i32;
    let max_lon_c = job.bounds.max_lon.floor() as i32;
    let cells: Vec<(i32, i32)> = (min_lat_c..=max_lat_c)
        .flat_map(|lat| (min_lon_c..=max_lon_c).map(move |lon| (lat, lon)))
        .collect();
    let total = cells.len() as u32;
    set_cell_progress(0, total);

    let mut combined = String::new();
    for (idx, (lat_c, lon_c)) in cells.iter().enumerate() {
        let done = idx as u32;
        set_cell_progress(done, total);

        let extract_path = extract_dir.join(format!("cell_{:+04}_{:+05}.osm.pbf", lat_c, lon_c));
        if !extract_path.exists() {
            let bbox = format!("{},{},{},{}", lon_c, lat_c, lon_c + 1, lat_c + 1);
            update_job_note(db_path, job.id,
                &format!("Osmium extract cell {}/{total} ({lat_c}°,{lon_c}°) — one-time, ~2-5 min…",
                         done + 1))?;
            let status = Command::new(&osmium)
                .arg("extract").arg("-b").arg(&bbox)
                .arg(&job.source_path).arg("-o").arg(&extract_path).arg("--overwrite")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status().map_err(|e| format!("Failed to launch osmium: {e}"))?;
            if !status.success() {
                let _ = fs::remove_file(&extract_path);
                clear_cell_progress();
                return Err(format!("osmium extract failed for cell ({lat_c},{lon_c})"));
            }
        } else {
            update_job_note(db_path, job.id,
                &format!("Scanning cached cell {}/{total} ({lat_c}°,{lon_c}°)…", done + 1))?;
        }

        let mut scan_job = job.clone();
        scan_job.source_path = extract_path;
        let result = import_focus_roads_via_stream_scan(db_path, &scan_job)?;
        if !combined.is_empty() { combined.push_str("; "); }
        combined.push_str(&result);
        // Increment gen so the render cache picks up each cell's data immediately.
        road_data_gen().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::app::request_repaint();
        set_cell_progress(done + 1, total);
    }

    clear_cell_progress();
    Ok(combined)
}
