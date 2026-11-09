use super::*;
use std::time::{Duration, Instant};

fn inventory(count: usize) -> Inventory {
    Inventory {
        terrain: terrain_assets::TerrainInventory {
            srtm_tiles: count,
            ..Default::default()
        },
        osm: Default::default(),
        runtime: Err("private test without a database".into()),
        layout_error: None,
    }
}

#[test]
fn blocked_inventory_does_not_block_polling_and_settings_changes_reject_old_results() {
    let root = Some(PathBuf::from("original"));
    let mut scan = Scan::new(root.clone());
    assert!(
        scan.pending.is_none(),
        "constructing the model must not scan or wait on disk"
    );
    let (started, entered) = mpsc::channel();
    let (release, held) = mpsc::channel();
    assert!(
        scan.poll_with(move |_| {
            started.send(()).unwrap();
            held.recv().unwrap();
            inventory(1)
        })
        .is_none()
    );
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    for _ in 0..100 {
        assert!(scan.poll_with(|_| panic!("duplicate worker")).is_none());
        assert!(scan.busy());
    }
    scan.request(Some(PathBuf::from("other")));
    // A settings change can alter paths even when selected_root is unchanged.
    scan.request(root.clone());
    assert!(
        scan.poll_with(|_| panic!("old worker gate released too early"))
            .is_none()
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let expected = root.clone();
        if let Some(outcome) = scan.poll_with(move |actual| {
            assert_eq!(actual, expected, "only the latest request is scanned");
            inventory(2)
        }) {
            assert_eq!(outcome.unwrap().terrain.srtm_tiles, 2);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(!scan.busy());
    assert!(
        scan.poll_with(|_| panic!("finished scan was repeated"))
            .is_none()
    );
}

#[test]
fn inventory_worker_failure_is_reported_and_can_be_refreshed() {
    let mut scan = Scan::new(None);
    scan.poll_with(|_| panic!("simulated scan failure"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = scan.poll_with(|_| panic!("duplicate worker")) {
            assert!(matches!(result, Err(error) if error.contains("panicked")));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(!scan.busy());
    scan.request(None);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = scan.poll_with(|_| inventory(3)) {
            assert_eq!(result.unwrap().terrain.srtm_tiles, 3);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}
