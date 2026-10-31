//! Remount regression and opt-in recovery verification against a real index copy.
use super::*;
use serde_json::Value;

fn state(index: &Path) -> Value {
    let db = rusqlite::Connection::open(index.join("nodes.sqlite")).unwrap();
    let text: String = db
        .query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))
        .unwrap();
    serde_json::from_str(&text).unwrap()
}
fn write_state(index: &Path, value: &Value) {
    let db = rusqlite::Connection::open(index.join("nodes.sqlite")).unwrap();
    db.execute("UPDATE state SET json=?1 WHERE id=1", [value.to_string()])
        .unwrap();
}
fn rejected(source: &Path, index: &Path) -> String {
    let before = fs::read(index.join("nodes.sqlite")).unwrap();
    let error = match PbfNodes::open(source, index, &mut |_| {}) {
        Ok(_) => panic!("invalid source accepted"),
        Err(error) => error,
    };
    assert_eq!(
        fs::read(index.join("nodes.sqlite")).unwrap(),
        before,
        "rejection must not alter saved work"
    );
    error
}
#[test]
fn remount_reuses_legacy_and_fingerprinted_indexes_without_losing_checkpoints() {
    for legacy in [false, true] {
        let temp = Scratch::new();
        let source = temp.source(include_bytes!(
            "../../tests/fixtures/planet-tiny-dense.osm.pbf"
        ));
        let (lookup, mut guard) = PbfNodes::open(&source, &temp.index(), &mut |_| {}).unwrap();
        let ids = [10, 20, 30, 40];
        let expected = lookup.session().lookup_many(&ids).unwrap();
        guard
            .bind_build("unchanged feature/output binding".into())
            .unwrap();
        drop(lookup);
        drop(guard);
        let sentinel = temp.index().join("ways_checkpoint.txt");
        fs::write(&sentinel, b"saved way offset").unwrap();
        let mut old = state(&temp.index());
        old["source"]["device"] = Value::from(old["source"]["device"].as_u64().unwrap() + 4);
        if legacy {
            old.as_object_mut().unwrap().remove("source_fingerprint");
        }
        write_state(&temp.index(), &old);
        let (lookup, guard) = PbfNodes::open(&source, &temp.index(), &mut |_| {
            panic!("completed index must not rescan")
        })
        .unwrap();
        assert_eq!(lookup.session().lookup_many(&ids).unwrap(), expected);
        drop(guard);
        let new = state(&temp.index());
        for key in [
            "offset",
            "nodes",
            "indexed",
            "first_way",
            "build_done",
            "build_options",
        ] {
            assert_eq!(new[key], old[key], "lost {key}");
        }
        assert_eq!(
            new["source"]["device"],
            Value::from(Stamp::read(&source).unwrap().device)
        );
        assert!(
            new["source_fingerprint"]
                .as_str()
                .unwrap()
                .starts_with("sample-v1:")
        );
        assert_eq!(fs::read(sentinel).unwrap(), b"saved way offset");
    }
}
#[test]
fn remount_does_not_relax_inode_size_or_timestamp_guards() {
    let temp = Scratch::new();
    let source = temp.source(include_bytes!(
        "../../tests/fixtures/planet-tiny-dense.osm.pbf"
    ));
    drop(PbfNodes::open(&source, &temp.index(), &mut |_| {}).unwrap());
    let original = state(&temp.index());
    for field in ["inode", "len", "seconds", "nanos"] {
        let mut saved = original.clone();
        saved["source"][field] = Value::from(saved["source"][field].as_i64().unwrap() + 1);
        saved["source"]["device"] = Value::from(0);
        write_state(&temp.index(), &saved);
        assert!(rejected(&source, &temp.index()).contains("source differs"));
    }
}
#[test]
fn content_fingerprint_catches_changed_bytes_even_with_restored_metadata() {
    let temp = Scratch::new();
    let source = temp.source(include_bytes!(
        "../../tests/fixtures/planet-tiny-dense.osm.pbf"
    ));
    drop(PbfNodes::open(&source, &temp.index(), &mut |_| {}).unwrap());
    let stamp = Stamp::read(&source).unwrap();
    let modified = fs::metadata(&source).unwrap().modified().unwrap();
    let mut bytes = fs::read(&source).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    fs::write(&source, bytes).unwrap();
    File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(Stamp::read(&source).unwrap(), stamp);
    assert!(rejected(&source, &temp.index()).contains("content fingerprint differs"));
}
#[test]
fn legacy_upgrade_checks_saved_descriptors_against_actual_source_blocks() {
    let temp = Scratch::new();
    let source = temp.source(&[header(), node_block(10), node_block(20)].concat());
    drop(PbfNodes::open(&source, &temp.index(), &mut |_| {}).unwrap());
    let mut saved = state(&temp.index());
    saved.as_object_mut().unwrap().remove("source_fingerprint");
    saved["source"]["device"] = Value::from(0);
    // Structurally valid descriptor/state, but the source contains only one node.
    saved["nodes"] = Value::from(3);
    write_state(&temp.index(), &saved);
    let db = rusqlite::Connection::open(temp.index().join("nodes.sqlite")).unwrap();
    db.execute("UPDATE blocks SET count=2 WHERE min_id=10", [])
        .unwrap();
    drop(db);
    assert!(rejected(&source, &temp.index()).contains("Cannot verify legacy compact index"));
}

#[test]
#[ignore = "requires explicit ONEKEE_RECOVERY_PBF and ONEKEE_RECOVERY_INDEX; reads original, upgrades only a temporary copy"]
fn real_index_copy_recovers_without_rebuilding() {
    let source = PathBuf::from(std::env::var_os("ONEKEE_RECOVERY_PBF").expect("PBF path"));
    let index = PathBuf::from(std::env::var_os("ONEKEE_RECOVERY_INDEX").expect("index folder"));
    let lock = File::open(index.join("build.lock")).unwrap();
    lock.try_lock()
        .expect("stop competing builds before verifying copy");
    let temp = Scratch::new();
    fs::create_dir_all(temp.index()).unwrap();
    fs::copy(
        index.join("nodes.sqlite"),
        temp.index().join("nodes.sqlite"),
    )
    .unwrap();
    let before = state(&temp.index());
    let (lookup, _guard) = PbfNodes::open(&source, &temp.index(), &mut |_| {
        panic!("must reuse complete index")
    })
    .unwrap();
    assert_eq!(lookup.node_count, before["nodes"].as_u64().unwrap());
    assert_eq!(lookup.first_way, before["first_way"].as_u64().unwrap());
    println!(
        "Verified remount recovery: {} nodes, {} blocks, first way offset {}",
        lookup.node_count,
        lookup.block_count(),
        lookup.first_way
    );
}
