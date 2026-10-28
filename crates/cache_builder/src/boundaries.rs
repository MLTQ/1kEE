//! Boundary-only entry point for desktop background jobs. No companion process.
pub use crate::roads::RoadBuildProgress as Progress;
use std::path::Path;

/// Build/resume administrative caches using the original PBF for coordinates.
/// The compact index lock prevents a second writer, including the companion CLI.
/// General way-build options/checkpoints are deliberately left untouched.
pub fn load(
    source: &Path,
    output: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<usize, String> {
    let crate::args::Command::PlanetAll(command) = crate::args::parse([
        "planet-all".into(),
        "--planet".into(),
        source.to_string_lossy().into_owned(),
        "--out-dir".into(),
        output.to_string_lossy().into_owned(),
        "--features".into(),
        "admin".into(),
        "--node-storage".into(),
        "indexed-pbf".into(),
    ])?
    else {
        unreachable!()
    };
    let scratch = command.tmp_dir.join("indexed-pbf-v1");
    let (nodes, _guard) = crate::pbf_nodes::PbfNodes::open(source, &scratch, &mut |p| {
        progress(Progress {
            stage: "Indexing PBF nodes".into(),
            fraction: 0.4 * p.bytes as f32 / p.total.max(1) as f32,
            message: format!(
                "{:.2} / {:.2} GB · {} nodes",
                p.bytes as f64 / 1e9,
                p.total as f64 / 1e9,
                p.nodes
            ),
        });
    })?;
    crate::admin::build_planet(
        &command,
        &crate::planet_lookup::Lookup::Compact(nodes),
        progress,
    )
}

/// Read-only probe of the OS lock, not the existence of the durable lock file.
pub fn is_running(output: &Path) -> Result<bool, String> {
    let path = output.join(".planet_build/indexed-pbf-v1/build.lock");
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}
