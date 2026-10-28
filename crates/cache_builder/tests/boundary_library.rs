//! Real PBF → shared desktop API → complete per-level caches and safe resume.
use one_thousand_electric_eye_cache_builder::boundaries;
use std::{fs, path::PathBuf};
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn desktop_import_writes_levels_resumes_and_respects_companion_lock() {
    let root = Scratch(std::env::temp_dir().join(
        format!("boundary-library-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()),
    ));
    fs::create_dir_all(&root.0).unwrap();
    let source = root.0.join("source.pbf");
    fs::write(&source, include_bytes!("fixtures/admin-tiny.osm.pbf")).unwrap();
    let output = root.0.join("output");
    assert!(!boundaries::is_running(&output).unwrap());
    let mut stages = Vec::new();
    let count = boundaries::load(&source, &output, &mut |p| stages.push(p.stage)).unwrap();
    assert!(count > 0);
    assert!(stages.iter().any(|s| s == "Assembling Admin Boundaries"));
    let read = || {
        (2..=11)
            .map(|n| {
                fs::read(
                    output
                        .join("admin_cells")
                        .join(cell_format::admin_filename(n)),
                )
                .unwrap()
            })
            .collect::<Vec<_>>()
    };
    let first = read();
    for bytes in &first {
        assert!(cell_format::read::read_single_chunk(bytes, cell_format::TAG_ADMN).is_some());
    }
    let index = output.join(".planet_build/indexed-pbf-v1");
    let conn = rusqlite::Connection::open(index.join("nodes.sqlite")).unwrap();
    // Boundary imports must not change or complete an unrelated way-build job.
    let saved: String = conn
        .query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))
        .unwrap();
    let state: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(state["build_done"], false);
    assert!(state["build_options"].is_null());
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(index.join("build.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(boundaries::is_running(&output).unwrap());
    assert!(
        boundaries::load(&source, &output, &mut |_| {})
            .unwrap_err()
            .contains("Cannot lock")
    );
    drop(lock);
    assert!(
        !boundaries::is_running(&output).unwrap(),
        "stale lock file is harmless"
    );
    assert_eq!(
        boundaries::load(&source, &output, &mut |_| {}).unwrap(),
        count
    );
    assert_eq!(read(), first);
    let again: String = conn
        .query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(again, saved);
    // Changed input preserves the existing cache and gives an actionable error.
    fs::write(&source, b"different PBF").unwrap();
    assert!(
        boundaries::load(&source, &output, &mut |_| {})
            .unwrap_err()
            .contains("source differs")
    );
    assert_eq!(read(), first);
}
