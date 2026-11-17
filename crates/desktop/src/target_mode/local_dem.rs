//! Read retained, numeric ground DEMs before asking a remote elevation service.
use super::{Terrain, project, terrain};
use lidar_pipeline::region::Region;
use rusqlite::{Connection, OpenFlags, params};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn path(root: &Path, value: &str) -> Option<PathBuf> {
    let p = Path::new(value);
    if p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let full = root.join(p).canonicalize().ok()?;
    (full.starts_with(&root) && full.is_file()).then_some(full)
}
fn metre_heights(survey: &str, band: &serde_json::Value) -> bool {
    let unit = band["unit"].as_str().unwrap_or("").to_ascii_lowercase();
    matches!(unit.as_str(),"m"|"metre"|"meter"|"metres"|"meters")
        // Verified publisher units, not inferred from horizontal projection:
        // https://portal.opentopography.org/datasetMetadata?otCollectionID=OT.102016.2039.1
        || survey=="opentopoID:OTLAS.102016.2039.1"
}

pub(super) fn load(
    catalog: &Path,
    region: Region,
    active: &Path,
) -> Result<Option<Terrain>, String> {
    let Some(root) = catalog.parent() else {
        return Ok(None);
    };
    let bulk = root.join("bulk.sqlite");
    if !catalog.is_file() || !bulk.is_file() {
        return Ok(None);
    }
    let db = Connection::open_with_flags(&bulk, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let surveys = Connection::open_with_flags(catalog, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let [w, s, e, n] = region.bounds();
    let mut statement=db.prepare("SELECT survey,path FROM files WHERE kind='raster' AND west<?1 AND east>?2 AND south<?3 AND north>?4 LIMIT 64").map_err(|e|e.to_string())?;
    let mut candidates = statement
        .query_map(params![e, w, n, s], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|(id, file)| {
            let p = path(root, &file)?;
            let (name, date): (String, String) = surveys
                .query_row(
                    "SELECT name,coalesce(date,'') FROM surveys WHERE id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok()?;
            Some((date, id, name, p))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    for (date, id, name, file) in candidates {
        if !active.is_file() {
            return Err("Target closed".into());
        }
        let metadata = temp.path().join("metadata.json");
        let mut command = Command::new(
            crate::panels::world_map::srtm_focus_cache::gdal::gdal_tool_path("gdalinfo"),
        );
        command
            .args(["-json"])
            .arg(&file)
            .env("PROJ_NETWORK", "OFF")
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&metadata).map_err(|e| e.to_string())?);
        if project::run(command, active, "Local DEM metadata").is_err() {
            continue;
        }
        if metadata.metadata().map_err(|e| e.to_string())?.len() > 1024 * 1024 {
            continue;
        }
        let info: serde_json::Value =
            serde_json::from_slice(&std::fs::read(metadata).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let Some(bands) = info["bands"].as_array() else {
            continue;
        };
        if bands.len() != 1
            || !matches!(
                bands[0]["type"].as_str(),
                Some("Float32" | "Float64" | "Int16" | "Int32")
            )
            || !metre_heights(&id, &bands[0])
        {
            continue;
        }
        let output = temp.path().join("target.tif");
        match crate::elevation_sources::local_target(&file, region, 2048, &output) {
            Ok(()) => {}
            Err(crate::elevation_sources::Error::NoCoverage) => continue,
            Err(crate::elevation_sources::Error::Failed(e)) => return Err(e),
        }
        if !active.is_file() {
            return Err("Target closed".into());
        }
        let native = if id == "opentopoID:OTLAS.102016.2039.1" {
            info["geoTransform"][1]
                .as_f64()
                .map(|s| format!(" · native cells ~{:.2} m", s.abs()))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let label =
            format!("{name} · {date} · retained LiDAR DTM{native} · vertical datum unverified");
        if let Ok(mut terrain) = terrain::read(&output, region, &label) {
            // Horizontal reprojection does not establish a matching Z reference.
            terrain.navd88 = false;
            return Ok(Some(terrain));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_dem_paths_and_z_units_are_not_inferred_from_xy() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("terrain.tif"), b"fixture").unwrap();
        assert!(path(root.path(), "terrain.tif").is_some());
        assert!(path(root.path(), "../terrain.tif").is_none());
        assert!(path(root.path(), "/tmp/terrain.tif").is_none());
        assert!(!metre_heights(
            "unknown",
            &serde_json::json!({"type":"Float32"})
        ));
        assert!(!metre_heights("unknown", &serde_json::json!({"unit":"ft"})));
        assert!(metre_heights("unknown", &serde_json::json!({"unit":"m"})));
        assert!(metre_heights(
            "opentopoID:OTLAS.102016.2039.1",
            &serde_json::json!({})
        ));
    }
    #[test]
    #[ignore = "reads the real ONEKEE_TARGET_TEST_CATALOG and retained Israel DEM"]
    fn live_target_israel_retained_dem() {
        let catalog = PathBuf::from(std::env::var_os("ONEKEE_TARGET_TEST_CATALOG").unwrap());
        let active = tempfile::NamedTempFile::new().unwrap();
        let region = Region {
            lon: 34.84,
            lat: 32.30,
            radius_m: 1000.,
        };
        let terrain = load(&catalog, region, active.path())
            .unwrap()
            .expect("retained Israel terrain");
        assert!(!terrain.indices.is_empty());
        assert!(terrain.label.contains("Israel Mediterranean Sharon"));
        assert!(terrain.label.contains("native cells ~2.03 m"));
        assert!(!terrain.navd88);
        let valid = terrain
            .heights
            .iter()
            .filter(|h| h.is_finite() && **h > -12000. && **h < 10000.)
            .count();
        eprintln!(
            "ISRAEL_DEM {} · valid={} triangles={}",
            terrain.label,
            valid,
            terrain.indices.len() / 3
        );
        assert!(valid > 1000);
    }
}
