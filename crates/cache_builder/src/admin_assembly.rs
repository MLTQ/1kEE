//! Country-first assembly with recoverable region commits and live publication.
use super::{
    checkpoint::Checkpoint,
    geometry::{self, CoordinateResolver},
};
use crate::{node_store::NodeStore, roads::RoadBuildProgress};
use serde::Serialize;
use std::{
    path::Path,
    time::{Duration, Instant},
};
#[derive(Serialize)]
struct Level {
    level: u8,
    assembled: usize,
    published: usize,
    total: usize,
}

pub(super) fn run(
    output: &Path,
    scratch: &Path,
    nodes: &NodeStore,
    resolve: &mut CoordinateResolver<'_>,
    validate: &dyn Fn() -> Result<(), String>,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<usize, String> {
    progress(RoadBuildProgress {
            metrics: None,stage:"Preparing boundary assembly".into(),fraction:0.90,
        message:"Preparing resumable country-first assembly; completed regions publish while extraction continues.".into()});
    nodes.prepare_admin_assembly()?;
    let source = format!("assembly-v1:{}", nodes.admin_source_identity()?);
    let mut saved = Checkpoint::open(&scratch.join("admin-assembly-v1.sqlite"), &source)?;
    let mut levels = Vec::new();
    let mut pending = Vec::new();
    for level in 2..=11 {
        let relations = nodes.admin_relations_at_level(level)?;
        let done = saved.completed(level)?;
        levels.push(Level {
            level,
            assembled: done.len(),
            published: done.len(),
            total: relations.len(),
        });
        pending.push(
            relations
                .into_iter()
                .filter(|(id, _)| !done.contains(id))
                .collect::<Vec<_>>(),
        );
    }
    validate()?;
    // Republishing makes a crash between region commit and rename harmless,
    // and supports switching output folders without repeating coordinate work.
    let mut ring_counts = [0usize; 10];
    for (i, level) in levels.iter().enumerate() {
        ring_counts[i] = saved.publish(output, level.level)?;
    }
    status(output, &levels)?;
    let total: usize = levels.iter().map(|l| l.total).sum();
    let mut completed: usize = levels.iter().map(|l| l.assembled).sum();
    for (i, relations) in pending.into_iter().enumerate() {
        let mut last_publish = Instant::now();
        for (id, name) in relations {
            progress(RoadBuildProgress {
                metrics: None,
                stage: "Assembling Admin Boundaries".into(),
                fraction: 0.90 + 0.09 * completed as f32 / total.max(1) as f32,
                message: format!(
                    "{completed} / {total} regions saved; resolving {} (relation {id}, level {})",
                    name.as_deref().unwrap_or("unnamed"),
                    levels[i].level
                ),
            });
            let members = nodes.admin_member_ids(id)?;
            let ways = nodes.admin_way_refs(&members)?;
            let mut region =
                geometry::assemble(id, name, &ways, &mut |ids| saved.resolve(ids, resolve))?;
            region.complete &= ways.len() == members.len();
            validate()?;
            saved.save(levels[i].level, &region)?;
            completed += 1;
            levels[i].assembled += 1;
            if levels[i].published == 0 || last_publish.elapsed() >= Duration::from_secs(30) {
                ring_counts[i] = saved.publish(output, levels[i].level)?;
                levels[i].published = levels[i].assembled;
                status(output, &levels)?;
                last_publish = Instant::now();
                progress(RoadBuildProgress {
                    metrics: None,
                    stage: "Published Admin Boundaries".into(),
                    fraction: 0.90 + 0.09 * completed as f32 / total.max(1) as f32,
                    message: format!(
                        "Level {}: {} / {} regions available in the app; extraction continues",
                        levels[i].level, levels[i].published, levels[i].total
                    ),
                });
            }
        }
        ring_counts[i] = saved.publish(output, levels[i].level)?;
        levels[i].published = levels[i].assembled;
        status(output, &levels)?;
    }
    Ok(ring_counts.iter().sum())
}
fn status(output: &Path, levels: &[Level]) -> Result<(), String> {
    let path = output.join("admin_cells/build-status.json");
    let tmp = path.with_extension("json.tmp");
    let value = serde_json::json!({"version":1,"complete":levels.iter().all(|l|l.published==l.total),"levels":levels});
    std::fs::write(&tmp, serde_json::to_vec(&value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(tmp, path).map_err(|e| e.to_string())
}
