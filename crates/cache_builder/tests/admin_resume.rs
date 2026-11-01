//! Crash/restart the public importer after its first country is already visible.
use one_thousand_electric_eye_cache_builder::boundaries;
use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
};
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn level(root: &Path, n: u8) -> Vec<u8> {
    fs::read(
        root.join("admin_cells")
            .join(cell_format::admin_filename(n)),
    )
    .unwrap()
}
#[test]
fn interruption_keeps_published_country_and_resume_skips_assembled_geometry() {
    let temp = Scratch(std::env::temp_dir().join(format!(
            "admin-resume-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    fs::create_dir_all(&temp.0).unwrap();
    let source = temp.0.join("source.pbf");
    let output = temp.0.join("out");
    fs::write(&source, include_bytes!("fixtures/admin-tiny.osm.pbf")).unwrap();
    let stopped = catch_unwind(AssertUnwindSafe(|| {
        boundaries::load(&source, &output, &mut |p| {
            if p.stage == "Published Admin Boundaries" {
                assert!(
                    p.message.contains("Level 2:"),
                    "countries must publish first"
                );
                panic!("simulate interruption after first publication");
            }
        })
        .unwrap();
    }));
    assert!(stopped.is_err());
    let country = level(&output, 2);
    let features = cell_format::read::read_single_chunk(&country, cell_format::TAG_ADMN).unwrap();
    assert_eq!(
        features.len(),
        3,
        "outer, hole and island already available"
    );
    assert!(features.iter().all(|f| f.is_polygon));
    assert!(
        cell_format::read::read_single_chunk(&level(&output, 4), cell_format::TAG_ADMN)
            .unwrap()
            .is_empty()
    );
    let status: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("admin_cells/build-status.json")).unwrap())
            .unwrap();
    assert_eq!(status["complete"], false);
    let dbpath = output.join(".planet_build/indexed-pbf-v1/admin-assembly-v1.sqlite");
    let db = rusqlite::Connection::open(&dbpath).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM regions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let coordinates: i64 = db
        .query_row("SELECT count(*) FROM coordinates", [], |r| r.get(0))
        .unwrap();
    assert!(coordinates > 0);
    drop(db);
    let mut resolving = Vec::new();
    boundaries::load(&source, &output, &mut |p| {
        if p.stage == "Assembling Admin Boundaries" {
            resolving.push(p.message);
        }
    })
    .unwrap();
    assert!(
        resolving.iter().all(|s| !s.contains("(relation 2,")),
        "completed country must never resolve again"
    );
    assert_eq!(level(&output, 2), country);
    let status: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("admin_cells/build-status.json")).unwrap())
            .unwrap();
    assert_eq!(status["complete"], true);
    // Removing an output file exercises republishing committed geometry.
    fs::remove_file(output.join("admin_cells/admin_level_2.1kc")).unwrap();
    boundaries::load(&source, &output, &mut |p| {
        assert_ne!(p.stage, "Assembling Admin Boundaries")
    })
    .unwrap();
    assert_eq!(level(&output, 2), country);
}
