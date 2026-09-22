//! Opt-in orchestration; legacy node files/checkpoint.txt are never opened here.
use super::{Checkpoint, run_pass2};
use crate::args::PlanetAllCommand;
use crate::pbf_nodes::PbfNodes;
use crate::planet_lookup::Lookup;
use crate::roads::RoadBuildProgress;
use std::fs;
use std::sync::Arc;

pub(super) fn build(
    cmd: &PlanetAllCommand,
    progress: &mut dyn FnMut(RoadBuildProgress),
) -> Result<String, String> {
    let directory = cmd.tmp_dir.join("indexed-pbf-v1");
    progress(RoadBuildProgress { stage: "Indexing original PBF".into(), fraction: 0.0,
        message: "Creating/reusing a compact block index; no planet_nodes.bin will be written. Existing flat-build files are kept.".into() });
    let (nodes, mut state) = PbfNodes::open(&cmd.planet_path, &directory, &mut |p| {
        progress(RoadBuildProgress {
            stage: "Indexing original PBF".into(),
            fraction: 0.4 * p.bytes as f32 / p.total.max(1) as f32,
            message: format!(
                "{:.2}/{:.2} GB scanned; {} nodes indexed in {} compressed blocks",
                p.bytes as f64 / 1e9,
                p.total as f64 / 1e9,
                p.nodes,
                p.blocks
            ),
        });
    })
    .map_err(|e| format!("Compact node index {}: {e}", directory.display()))?;
    let options = serde_json::json!({
        "output": fs::canonicalize(&cmd.out_dir).map_err(|e| e.to_string())?,
        "elevation_source": cmd.srtm_root.as_ref().map(fs::canonicalize).transpose().map_err(|e| e.to_string())?,
        "features": [cmd.build_roads, cmd.build_waterways, cmd.build_buildings, cmd.build_trees,
            cmd.build_admin, cmd.build_power, cmd.build_rail, cmd.build_pipeline, cmd.build_aeroway,
            cmd.build_military, cmd.build_comm, cmd.build_industrial, cmd.build_port, cmd.build_government,
            cmd.build_surveillance],
        "pipeline": 1,
    }).to_string();
    if state.bind_build(options)? {
        return Ok("Compact build already completed for these source/settings. Existing output and legacy files were left intact. Use a new working directory to rebuild.".into());
    }
    let checkpoint_path = directory.join("ways_checkpoint.txt");
    let mut cp = Checkpoint::load(&checkpoint_path);
    let length = fs::metadata(&cmd.planet_path)
        .map_err(|e| e.to_string())?
        .len();
    if cp.pass2_offset > length {
        return Err("Compact feature checkpoint exceeds the PBF length".into());
    }
    cp.pass2_offset = cp.pass2_offset.max(nodes.first_way);
    progress(RoadBuildProgress {
        stage: "Compact node index ready".into(),
        fraction: 0.4,
        message: format!(
            "{} nodes, {} blocks; {:.2} MB index on disk. Reading needed coordinates from the original PBF.",
            nodes.node_count,
            nodes.block_count(),
            fs::metadata(directory.join("nodes.sqlite"))
                .map_err(|e| e.to_string())?
                .len() as f64
                / 1e6
        ),
    });
    let lookup = Arc::new(Lookup::Compact(nodes));
    let stats = run_pass2(cmd, &lookup, &mut cp, &checkpoint_path, progress)?;
    lookup.validate_source()?;
    state.finish()?;
    let summary = format!(
        "Compact planet build: {} features; {} cache files written. No expanded planet_nodes.bin created.{}",
        stats.feature_count,
        stats.written_cells,
        if cmd.build_admin {
            " Admin relations remain unsupported in planet-all."
        } else {
            ""
        }
    );
    progress(RoadBuildProgress {
        stage: "Completed".into(),
        fraction: 1.0,
        message: summary.clone(),
    });
    Ok(summary)
}
