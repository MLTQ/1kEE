use super::*;

fn seed(path: &Path, count: i64) {
    let connection = db::open_cache_db(path).unwrap();
    connection
        .execute(
            "INSERT INTO contour_tile_manifest VALUES(0,1,-2,?1,0)",
            [count],
        )
        .unwrap();
}

#[test]
fn cache_diagnostics_never_convert_database_errors_to_zero_and_preserve_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("srtm_native_v1.sqlite");
    let bytes = b"damaged SQLite header";
    fs::write(&path, bytes).unwrap();
    assert!(matches!(original(&path), Original::Unreadable(_)));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    // A schema/query failure is also an error, never an empty-cache success.
    let other = dir.path().join("wrong-schema.sqlite");
    rusqlite::Connection::open(&other)
        .unwrap()
        .execute_batch("CREATE TABLE unrelated(id INTEGER)")
        .unwrap();
    assert!(matches!(original(&other), Original::Unreadable(_)));
}

#[test]
fn cache_diagnostics_distinguish_genuine_empty_tiles_from_empty_or_missing_caches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("srtm_native_v1.sqlite");
    assert!(matches!(original(&path), Original::Missing));
    assert!(!path.exists());
    drop(db::open_cache_db(&path).unwrap());
    assert!(matches!(original(&path), Original::Ready(levels) if levels.is_empty()));
    seed(&path, 0);
    match original(&path) {
        Original::Ready(levels) => {
            assert_eq!(levels.len(), 1);
            assert_eq!(
                (levels[0].zoom, levels[0].tiles, levels[0].lines),
                (0, 1, 0)
            );
        }
        other => panic!("valid empty tile reported as {other:?}"),
    }
}

#[test]
fn cache_diagnostics_find_recovery_while_original_totals_are_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("srtm_native_v1.sqlite");
    let bytes = b"original must remain untouched";
    fs::write(&base, bytes).unwrap();
    let root = dir.path().join("isolated-recovery");
    let repaired = root.join("0_1_-2/srtm_native_v1.sqlite");
    seed(&repaired, 3633);
    let cache = inspect("SRTM contours", &base, root.clone(), |_| repaired.clone());
    assert!(matches!(cache.original, Original::Unreadable(_)));
    assert!(matches!(cache.recovery, Recovery::Available {sample} if sample == repaired));
    assert_eq!(cache.recovery_root, root);
    assert_eq!(fs::read(&base).unwrap(), bytes);
}

#[test]
fn cache_diagnostics_markers_are_not_ready_and_nested_replacements_take_priority() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("srtm_native_v1.sqlite");
    let root = dir.path().join("repairs");
    let old = root.join("0_1_-2/srtm_native_v1.sqlite");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    assert!(matches!(
        recovered(&base, &root, |_| old.clone()),
        Recovery::Present
    ));
    seed(&old, 99);
    let nested = dir.path().join("nested/srtm_native_v1.sqlite");
    // The same resolver as rendering must be used, not an old repair file.
    assert!(matches!(
        recovered(&base, &root, |_| nested.clone()),
        Recovery::Present
    ));
    seed(&nested, 0);
    assert!(
        matches!(recovered(&base,&root,|_|nested.clone()),Recovery::Available {sample} if sample == nested)
    );
}

#[test]
fn cache_diagnostics_bound_recovery_probes_and_do_not_create_directories() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base.sqlite");
    let root = dir.path().join("repairs");
    assert!(matches!(
        recovered(&base, &root, |_| base.clone()),
        Recovery::Absent
    ));
    assert!(!root.exists());
    for lat in 0..100 {
        fs::create_dir_all(root.join(format!("0_{lat}_-2"))).unwrap();
    }
    let probes = std::cell::Cell::new(0);
    assert!(matches!(
        recovered(&base, &root, |_| {
            probes.set(probes.get() + 1);
            base.clone()
        }),
        Recovery::Present
    ));
    assert!(probes.get() > 0 && probes.get() <= 32);
}

#[test]
#[ignore = "reads only bounded cache metadata on the attached Minkowski terrain drive"]
fn cache_diagnostics_live_minkowski_recognizes_working_recovery() {
    let caches = load(Some(Path::new("/run/media/m/Minkowski")));
    for cache in caches {
        eprintln!("{cache:?}");
        assert!(matches!(cache.recovery, Recovery::Available { .. }));
        assert!(!matches!(cache.original,Original::Ready(ref levels) if levels.is_empty()));
    }
}
