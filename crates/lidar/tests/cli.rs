#![cfg(feature = "builder")]
use rusqlite::{Connection, params};
use std::{fs, path::Path, process::Command};

fn run(root: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_one-thousand-electric-eye-lidar"))
        .arg("--root")
        .arg(root)
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn cli_resumes_existing_receipts_verifies_output_and_rejects_wrong_volume() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut header = las::Builder::from(las::Version::new(1, 4))
        .into_header()
        .unwrap();
    // A standard geographic WKT; external coordinate infrastructure is not a test oracle.
    header.set_wkt_crs(b"GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563]],PRIMEM[\"Greenwich\",0],UNIT[\"degree\",0.0174532925199433],AUTHORITY[\"EPSG\",\"4326\"]]".to_vec()).unwrap();
    let raw = root.join("scan.las");
    let mut writer = las::Writer::from_path(&raw, header).unwrap();
    writer
        .write_point(las::Point {
            x: 0.,
            y: 0.,
            z: 12.,
            ..Default::default()
        })
        .unwrap();
    writer.close().unwrap();
    let catalog = Connection::open(root.join("catalog.sqlite")).unwrap();
    catalog.execute_batch("CREATE TABLE surveys(id TEXT,date TEXT,name TEXT,page TEXT);INSERT INTO surveys VALUES('scan','2026','Scan','https://example.org');").unwrap();
    let bulk = Connection::open(root.join("bulk.sqlite")).unwrap();
    bulk.execute_batch("CREATE TABLE files(url TEXT,survey TEXT,path TEXT,bytes INTEGER,kind TEXT,west REAL,south REAL,east REAL,north REAL);").unwrap();
    bulk.execute(
        "INSERT INTO files VALUES ('https://example.org/scan','scan','scan.las',?1,'copc',0,0,0,0)",
        params![raw.metadata().unwrap().len()],
    )
    .unwrap();
    let plan = run(root, &["--plan"]);
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(plan["files"], 1);
    let first = run(root, &[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&first.stdout).unwrap()["status"],
        "built"
    );
    let reused = run(root, &[]);
    assert!(
        reused.status.success(),
        "{}",
        String::from_utf8_lossy(&reused.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&reused.stdout).unwrap()["status"],
        "reused"
    );
    let audit = run(root, &["--verify"]);
    assert!(
        audit.status.success(),
        "{}",
        String::from_utf8_lossy(&audit.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&audit.stdout).unwrap()["verified"],
        1
    );
    if cfg!(target_os = "linux") {
        let wrong = run(root, &["--volume-uuid", "incorrect-volume"]);
        assert!(!wrong.status.success());
        assert!(String::from_utf8_lossy(&wrong.stderr).contains("identity mismatch"));
    }
    assert!(raw.is_file());
    let invalid = run(root, &["--max-files", "0"]);
    assert!(!invalid.status.success());
    let index = Connection::open(root.join("fused/v1/index.sqlite")).unwrap();
    let path: String = index
        .query_row("SELECT path FROM rasters", [], |r| r.get(0))
        .unwrap();
    assert!(Path::new(&path).is_relative());
    fs::write(root.join(path), "corrupt").unwrap();
    assert!(!run(root, &["--verify"]).status.success());
}
