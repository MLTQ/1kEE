//! Nonblocking cache observation and explicit, single-flight PBF imports.
use crate::model::AppModel;
use one_thousand_electric_eye_cache_builder::boundaries;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Instant, SystemTime},
};

pub(super) type Fingerprint = Vec<(PathBuf, u64, Option<SystemTime>)>;
#[derive(Default)]
pub(super) struct Status {
    pub files: Fingerprint,
    pub running: bool,
    pub error: Option<String>,
}
type Watch = super::layer_snapshot::Snapshot<(Vec<PathBuf>, u64), Status>;

pub(super) fn directories(model: &AppModel) -> Vec<PathBuf> {
    if let Some(root) = &model.admin_options.cache_root {
        return vec![root.clone()];
    }
    model
        .selected_root
        .as_deref()
        .map(super::cell_loader::osm_cache_dir_candidates)
        .unwrap_or_default()
}
pub(super) fn cache_root(path: &Path) -> PathBuf {
    if path.file_name().is_some_and(|n| n == "admin_cells") {
        path.parent().unwrap_or(path).to_owned()
    } else {
        path.to_owned()
    }
}

pub(super) fn poll(dirs: &[PathBuf]) -> Option<Arc<Status>> {
    static WATCH: OnceLock<Mutex<Watch>> = OnceLock::new();
    static START: OnceLock<Instant> = OnceLock::new();
    let epoch = START.get_or_init(Instant::now).elapsed().as_secs() / 3;
    WATCH.get_or_init(Default::default).lock().unwrap().get(
        (dirs.to_vec(), epoch),
        "admin-cache-watch",
        |a, b| a.0 == b.0,
        |(dirs, _)| Some(inspect(&dirs)),
    )
}
fn inspect(dirs: &[PathBuf]) -> Status {
    let mut status = Status::default();
    for root in dirs {
        match boundaries::is_running(root) {
            Ok(running) => status.running |= running,
            Err(e) => status.error = Some(e),
        }
        for level in 2..=11 {
            for extension in ["1kc", "geojson"] {
                let path = root
                    .join("admin_cells")
                    .join(format!("admin_level_{level}.{extension}"));
                match path.metadata() {
                    Ok(m) => status.files.push((path, m.len(), m.modified().ok())),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => status.error = Some(format!("{}: {e}", path.display())),
                }
            }
        }
    }
    status
}

#[derive(Clone)]
pub(super) struct Job {
    pub source: PathBuf,
    pub output: PathBuf,
    pub active: bool,
    pub fraction: f32,
    pub message: String,
}
fn job() -> &'static Mutex<Option<Job>> {
    static JOB: OnceLock<Mutex<Option<Job>>> = OnceLock::new();
    JOB.get_or_init(Default::default)
}
pub(super) fn current() -> Option<Job> {
    job().lock().unwrap().clone()
}
fn report(fraction: f32, message: String, active: bool) {
    if let Some(j) = job().lock().unwrap().as_mut() {
        j.fraction = fraction;
        j.message = message;
        j.active = active;
    }
    crate::app::request_repaint();
}
pub(super) fn start(source: PathBuf, output: PathBuf) {
    {
        let mut current = job().lock().unwrap();
        if current.as_ref().is_some_and(|j| j.active) {
            return;
        }
        *current = Some(Job {
            source: source.clone(),
            output: output.clone(),
            active: true,
            fraction: 0.0,
            message: "Opening boundary source…".into(),
        });
    }
    if let Err(error) = std::thread::Builder::new().name("admin-pbf-import".into()).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if boundaries::is_running(&output)? {
                return Ok("Using the existing extraction in this cache folder. Completed boundary files load automatically.".to_owned());
            }
            boundaries::load(&source, &output, &mut |p| report(p.fraction, format!("{}: {}", p.stage, p.message), true))
                .map(|n| format!("Loaded {n} boundary rings. Available in Globe and Local."))
        })).unwrap_or_else(|_| Err("Boundary import stopped unexpectedly. Scan checkpoints are retained; retry to resume.".into()));
        match result {
            Ok(message) => report(1.0, message, false),
            Err(error) => report(0.0, format!("Boundary import failed: {error}"), false),
        }
        super::admin_layer::reload();
    }) { report(0.0, format!("Cannot start boundary import: {error}"), false); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observes_new_and_replaced_cache_files_and_normalizes_admin_folder() {
        let root = std::env::temp_dir().join(format!("admin-watch-{}", std::process::id()));
        let dir = root.join("admin_cells");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(cache_root(&dir), root);
        let paths = [root.clone()];
        let before = inspect(&paths);
        std::fs::write(dir.join("admin_level_4.1kc"), [1, 2, 3]).unwrap();
        let after = inspect(&paths);
        assert_ne!(before.files, after.files);
        assert!(!after.running);
        std::fs::write(dir.join("admin_level_4.1kc"), [1, 2, 3, 4]).unwrap();
        assert_ne!(after.files, inspect(&paths).files);
        std::fs::remove_dir_all(root).unwrap();
    }
}
