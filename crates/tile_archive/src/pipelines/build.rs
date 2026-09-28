use super::{Info, MANIFEST, TAG, encoded, geometry, key};
use crate::Writer;
use cell_format::CellFeature;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Route {
    info: Info,
    points: Vec<[f64; 2]>,
}

struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(format!("{}-journal", self.0.display()));
    }
}

pub fn run(
    input: &Path,
    manifest: &Path,
    output: &Path,
    progress: &mut dyn FnMut(String),
) -> Result<String, String> {
    if output.exists() {
        return Err(format!("Destination already exists: {}", output.display()));
    }
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(manifest).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if manifest["version"] != 1 || !manifest["sources"].is_array() {
        return Err("Unsupported pipeline source manifest".into());
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let stage = Stage(parent.join(format!(".pipelines-{}-{nonce}.part", std::process::id())));
    let mut writer = Writer::create(&stage.0)?;
    let mut cells: BTreeMap<(i32, i32), Vec<CellFeature>> = BTreeMap::new();
    let mut overview = Vec::new();
    let mut id = 0i64;
    let mut routes = 0;
    let mut vertices = 0;
    let reader = BufReader::new(fs::File::open(input).map_err(|e| e.to_string())?);
    for (line, row) in reader.lines().enumerate() {
        let row = row.map_err(|e| e.to_string())?;
        if row.len() > 128 * 1024 * 1024 {
            return Err(format!("Pipeline row {} exceeds 128 MiB", line + 1));
        }
        let route: Route =
            serde_json::from_str(&row).map_err(|e| format!("Pipeline row {}: {e}", line + 1))?;
        if route.points.len() < 2
            || route.points.iter().any(|p| {
                !p[0].is_finite() || !p[1].is_finite() || p[0].abs() > 180.0 || p[1].abs() > 90.0
            })
        {
            return Err(format!("Invalid route coordinates at row {}", line + 1));
        }
        let name = serde_json::to_string(&route.info).map_err(|e| e.to_string())?;
        if name.len() > u16::MAX as usize {
            return Err("Pipeline attributes exceed cell codec limit".into());
        }
        let fragments = geometry::split(&route.points);
        // Join neighboring fragments only for the overview, breaking at the dateline.
        let mut overview_line = Vec::new();
        for (cell, points) in fragments {
            if !overview_line.is_empty() && overview_line.last() != points.first() {
                id += 1;
                overview.push(encoded(id, &name, geometry::simplify(&overview_line, 0.02)));
                overview_line.clear();
            }
            overview_line.extend(
                points
                    .iter()
                    .skip(usize::from(!overview_line.is_empty()))
                    .copied(),
            );
            id += 1;
            cells
                .entry(cell)
                .or_default()
                .push(encoded(id, &name, points));
        }
        if overview_line.len() >= 2 {
            id += 1;
            overview.push(encoded(id, &name, geometry::simplify(&overview_line, 0.02)));
        }
        routes += 1;
        vertices += route.points.len();
        if routes % 1000 == 0 {
            progress(format!(
                "Prepared {routes} route parts, {vertices} source vertices"
            ));
        }
    }
    if overview.is_empty() {
        return Err("No valid pipeline routes; archive not published".into());
    }
    let tile_count = cells.len();
    let mut batch = Vec::new();
    for ((y, x), features) in cells {
        batch.push((
            key(0, y, x),
            cell_format::write::write_cell(0, 0, &[(TAG, &features)]),
        ));
        if batch.len() >= 64 {
            writer.put_batch(&batch, None)?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        writer.put_batch(&batch, None)?;
    }
    let overview_vertices: usize = overview.iter().map(|f| f.points.len()).sum();
    writer.put_batch(
        &[(
            key(1, 0, 0),
            cell_format::write::write_cell(0, 0, &[(TAG, &overview)]),
        )],
        None,
    )?;
    manifest["archive"] = serde_json::json!({"route_parts":routes,"source_vertices":vertices,"tiles":tile_count,"overview_vertices":overview_vertices,"overview_tolerance_degrees":0.02});
    writer.metadata(MANIFEST, &manifest.to_string())?;
    writer.finish()?;
    fs::hard_link(&stage.0, output)
        .map_err(|e| format!("Cannot publish {}: {e}", output.display()))?;
    Ok(format!(
        "Published {routes} route parts in {tile_count} tiles; {vertices} full / {overview_vertices} overview vertices to {}",
        output.display()
    ))
}
