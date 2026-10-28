//! Real CLI/PBF regression; no external tooling needed at test time.
use cell_format::{TAG_ADMN, read::read_single_chunk};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "1kee-admin-build-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("source.pbf"),
            include_bytes!("fixtures/admin-tiny.osm.pbf"),
        )
        .unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn build(root: &Path, mode: &str) {
    let output = Command::new(env!(
        "CARGO_BIN_EXE_one-thousand-electric-eye-cache-builder"
    ))
    .args(["planet-all", "--planet"])
    .arg(root.join("source.pbf"))
    .arg("--out-dir")
    .arg(root.join(mode).join("out"))
    .arg("--tmp-dir")
    .arg(root.join(mode).join("tmp"))
    .args(["--features", "admin", "--node-storage", mode])
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("unsupported"));
}
#[test]
fn admin_pbf_flat_and_compact_build_all_levels_with_complete_holes() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    for mode in ["flat", "indexed-pbf"] {
        build(root, mode);
        let dir = root.join(mode).join("out/admin_cells");
        for level in 2..=11 {
            let bytes = fs::read(dir.join(format!("admin_level_{level}.1kc"))).unwrap();
            let rings = read_single_chunk(&bytes, TAG_ADMN).unwrap();
            let complete: Vec<_> = rings.iter().filter(|f| f.way_id == level).collect();
            assert_eq!(complete.len(), 3, "outer, hole, island at level {level}");
            assert!(
                complete
                    .iter()
                    .all(|f| f.is_polygon && f.class == level as u8)
            );
            if level == 4 {
                assert!(rings.iter().any(|f| f.way_id == 20));
                assert!(rings.iter().any(|f| f.way_id == 21));
                assert!(
                    rings
                        .iter()
                        .filter(|f| f.way_id >= 20)
                        .all(|f| !f.is_polygon)
                );
                assert_eq!(
                    rings.iter().filter(|f| f.way_id == 20).count(),
                    2,
                    "missing node splits border"
                );
            }
        }
    }
    for level in 2..=11 {
        let file = format!("out/admin_cells/admin_level_{level}.1kc");
        assert_eq!(
            fs::read(root.join("flat").join(&file)).unwrap(),
            fs::read(root.join("indexed-pbf").join(&file)).unwrap()
        );
    }
    // A focused import into a global cache cannot erase complete regions.
    let output = Command::new(env!(
        "CARGO_BIN_EXE_one-thousand-electric-eye-cache-builder"
    ))
    .args(["roads-bbox", "--planet"])
    .arg(root.join("source.pbf"))
    .arg("--cache-dir")
    .arg(root.join("flat/out"))
    .args([
        "--min-lat",
        "0",
        "--max-lat",
        "1",
        "--min-lon",
        "0",
        "--max-lon",
        "2",
        "--margin-deg",
        "0",
        "--features",
        "admin",
    ])
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(root.join("flat/out/admin_cells/admin_level_4.1kc")).unwrap();
    let features = read_single_chunk(&bytes, TAG_ADMN).unwrap();
    let preserved: Vec<_> = features.iter().filter(|f| f.way_id == 4).collect();
    assert_eq!(preserved.len(), 3);
    assert!(preserved.iter().all(|f| f.is_polygon));

    // Existing completed compact way builds must be able to backfill admin output.
    let dir = root.join("indexed-pbf/out/admin_cells");
    fs::remove_dir_all(&dir).unwrap();
    build(root, "indexed-pbf");
    assert!(dir.join("admin_level_4.1kc").exists());
}
