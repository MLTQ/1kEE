//! Opt-in production loading benchmark; no display or cache-builder job needed.
use super::*;
#[test]
#[ignore = "reads original terrain from ONEKEE_ATLAS_ROOT; reports production load latency"]
fn atlas_map_load_latency() {
    let root = PathBuf::from(std::env::var("ONEKEE_ATLAS_ROOT").expect("terrain root"));
    // Original terrain is read through a symlink; all new cache writes are private.
    let scratch = tempfile::tempdir().unwrap();
    std::fs::create_dir(scratch.path().join("Derived")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        terrain_assets::find_data_root(Some(&root)).unwrap(),
        scratch.path().join("Data"),
    )
    .unwrap();
    #[cfg(not(unix))]
    panic!("This native-source benchmark uses Unix symlinks to isolate its output");
    let root = scratch.path().to_owned();
    for (name, bounds) in [
        ("regional", [-6 * 3600, 40 * 3600, 20 * 3600, 55 * 3600]),
        ("local", [19 * 3600, 50 * 3600, 20 * 3600, 51 * 3600]),
    ] {
        let mut first_raster = None;
        for pass in 0..4 {
            if pass == 3 {
                super::super::atlas_source::flush_view_cache();
                *RECENT.get().unwrap().lock().unwrap() = RecentDetails::default();
            }
            let request = DetailRequest {
                root: Some(root.clone()),
                region: super::super::atlas_viewport::Region(bounds),
                max_edge: 8192,
                epoch: 0,
            };
            let started = std::time::Instant::now();
            let detail = load_detail(&request).unwrap();
            eprintln!(
                "ATLAS_BENCH {name} pass={pass} total_ms={:.3} size={:?} raster={}",
                started.elapsed().as_secs_f64() * 1000.,
                detail.raster.size,
                detail.raster.id
            );
            assert!(detail.raster.heights.iter().any(|h| h.is_finite()));
            if pass == 0 {
                first_raster = Some(detail.raster.clone());
            }
            if pass == 1 || pass == 2 {
                assert!(Arc::ptr_eq(first_raster.as_ref().unwrap(), &detail.raster));
            }
            if pass == 3 {
                assert_ne!(first_raster.as_ref().unwrap().id, detail.raster.id);
                assert_eq!(first_raster.as_ref().unwrap().colors, detail.raster.colors);
                assert!(
                    first_raster
                        .as_ref()
                        .unwrap()
                        .heights
                        .iter()
                        .zip(&detail.raster.heights)
                        .all(|(a, b)| a.to_bits() == b.to_bits())
                );
            }
        }
    }
    super::super::atlas_source::flush_view_cache();
    let database = scratch
        .path()
        .join("Derived/terrain/atlas_recent_views_v1.sqlite");
    let c = rusqlite::Connection::open(&database).unwrap();
    let (views, bytes): (u64, u64) = c
        .query_row("SELECT count(*),sum(length(v.payload)+length(m.payload)) FROM views v JOIN materials m USING(key)", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(views, 2);
    eprintln!(
        "ATLAS_BENCH persisted_views={views} compressed_MB={:.2}",
        bytes as f64 / 1e6
    );
}
