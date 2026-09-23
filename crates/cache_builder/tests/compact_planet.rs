use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "1kee-compact-integration-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn build(root: &Path, mode: &str, features: &str) -> Output {
    Command::new(env!(
        "CARGO_BIN_EXE_one-thousand-electric-eye-cache-builder"
    ))
    .args(["planet-all", "--planet"])
    .arg(root.join("source.pbf"))
    .arg("--out-dir")
    .arg(root.join(mode).join("out"))
    .arg("--tmp-dir")
    .arg(root.join(mode).join("tmp"))
    .args(["--features", features, "--node-storage", mode])
    .output()
    .unwrap()
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn compact_build_matches_flat_and_preserves_legacy_state() {
    for fixture in [
        include_bytes!("fixtures/planet-tiny-plain.osm.pbf").as_slice(),
        include_bytes!("fixtures/planet-tiny-dense.osm.pbf").as_slice(),
    ] {
        let scratch = Scratch::new();
        let root = &scratch.0;
        fs::write(root.join("source.pbf"), fixture).unwrap();
        let tmp = root.join("indexed-pbf/tmp");
        fs::create_dir_all(&tmp).unwrap();
        for file in [
            "planet_nodes.bin",
            "checkpoint.txt",
            "planet_nodes.sparse-index-v1",
        ] {
            fs::write(tmp.join(file), b"legacy state must remain untouched").unwrap();
        }
        for mode in ["flat", "indexed-pbf"] {
            let run = build(root, mode, "all");
            assert!(
                run.status.success(),
                "{}",
                String::from_utf8_lossy(&run.stderr)
            );
        }
        let expected = files(&root.join("flat/out"));
        assert_eq!(expected.len(), 2); // One road and one building; one feature per file.
        assert_eq!(files(&root.join("indexed-pbf/out")), expected);
        for file in [
            "planet_nodes.bin",
            "checkpoint.txt",
            "planet_nodes.sparse-index-v1",
        ] {
            assert_eq!(
                fs::read(tmp.join(file)).unwrap(),
                b"legacy state must remain untouched"
            );
        }
        let saved = files(&tmp);
        assert!(build(root, "indexed-pbf", "all").status.success());
        assert_eq!(files(&tmp), saved);
        assert_eq!(files(&root.join("indexed-pbf/out")), expected);

        let run = build(root, "indexed-pbf", "roads");
        assert!(!run.status.success());
        assert!(String::from_utf8_lossy(&run.stderr).contains("settings/output changed"));
        assert_eq!(files(&tmp), saved);

        fs::write(root.join("source.pbf"), [fixture, &[0]].concat()).unwrap();
        let run = build(root, "indexed-pbf", "all");
        assert!(!run.status.success());
        assert!(String::from_utf8_lossy(&run.stderr).contains("source differs"));
        assert_eq!(files(&tmp), saved);
        assert_eq!(files(&root.join("indexed-pbf/out")), expected);
    }
}

#[test]
fn corrupt_source_and_invalid_storage_do_not_create_feature_checkpoints() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("source.pbf"), b"corrupt PBF").unwrap();
    assert!(!build(&scratch.0, "indexed-pbf", "all").status.success());
    assert!(
        !scratch
            .0
            .join("indexed-pbf/tmp/indexed-pbf-v1/ways_checkpoint.txt")
            .exists()
    );
    let run = build(&scratch.0, "typo", "all");
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("--node-storage must be"));
    assert!(!scratch.0.join("typo").exists());
}
