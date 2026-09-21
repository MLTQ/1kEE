//! Explicit opt-in benchmarks: disposable output, never the user's cache.
use super::*;
use std::sync::atomic::AtomicUsize;

#[path = "bench_resources.rs"]
mod resources;

fn tile_count(default: usize) -> usize {
    std::env::var("ONEKEE_TERRAIN_BENCH_TILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| (1..=25).contains(n))
        .unwrap_or(default)
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(label: &str) -> Self {
        let parent = std::env::var_os("ONEKEE_TERRAIN_BENCH_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = parent.join(format!(
            "1kee-{label}-{}-{}",
            std::process::id(),
            unique_temp_token(),
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

#[test]
#[ignore = "needs GDAL; downloads one USGS raster unless ONEKEE_TERRAIN_BENCH_RASTER is supplied"]
fn benchmark_contour_processing_concurrency() {
    let scratch = Scratch::new("processing-bench");
    let raster = if let Some(path) = std::env::var_os("ONEKEE_TERRAIN_BENCH_RASTER") {
        PathBuf::from(path)
    } else {
        let path = scratch.0.join("source.tif");
        assert!(crate::threedep::fetch_tile_raster(
            40.0052, -105.3048, 40.0348, -105.2752, 2400, &path
        ));
        path
    };
    let mut reference = None;
    let tiles = tile_count(8);
    let worker_counts = std::env::var("ONEKEE_TERRAIN_BENCH_WORKERS")
        .map(|v| {
            v.split(',')
                .map(|n| n.parse::<usize>().expect("worker count"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|_| vec![2, 4, 6, 1, 4, 2]);
    assert!(worker_counts.iter().all(|n| (1..=25).contains(n)));
    // Alternate order to expose warm-cache/order bias. Every batch builds the
    // same raster copies and imports into a fresh, shared WAL database.
    for (round, workers) in worker_counts.into_iter().enumerate() {
        let cache = scratch.0.join(format!("run-{round}.sqlite"));
        super::super::db::open_cache_db(&cache).unwrap();
        let next = AtomicUsize::new(0);
        let resources = resources::Sampler::start();
        let start = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= tiles {
                            break;
                        }
                        let tile = TileKey {
                            zoom_bucket: 10,
                            lat_bucket: round as i32,
                            lon_bucket: index as i32,
                        };
                        let gpkg = scratch.0.join(format!("{round}-{index}.gpkg"));
                        let timer = StageTimer::new(tile, "contour");
                        run_gdal_contour(
                            &raster,
                            &gpkg,
                            0.5,
                            Some(crate::threedep::nodata_sentinel()),
                        )
                        .unwrap();
                        timer.finish(0, 0);
                        import_tile_into_cache(&cache, tile, &gpkg, None).unwrap();
                        fs::remove_file(gpkg).unwrap();
                    }
                });
            }
        });
        let elapsed = start.elapsed();
        let peak = resources.finish();
        let db = super::super::db::open_cache_db_read_only(&cache).unwrap();
        let counts: (i64, i64, i64) = db.query_row(
            "SELECT count(*),sum(length(geom)),(SELECT count(*) FROM contour_tile_manifest) FROM contour_tiles",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(counts.2, tiles as i64);
        if let Some(expected) = reference {
            assert_eq!(counts, expected);
        }
        reference = Some(counts);
        eprintln!(
            "PROCESSING workers={workers} tiles={tiles} wall_ms={:.1} rows={} bytes={} peak_rss_mib={:.1} peak_gdal={}",
            elapsed.as_secs_f64() * 1000.0,
            counts.0,
            counts.1,
            peak.rss_kib as f64 / 1024.0,
            peak.gdal_processes
        );
        // Keep storage bounded to one round, including on a nearly-full
        // external volume. Only this run's unique output files are removed.
        drop(db);
        fs::remove_file(&cache).unwrap();
    }
}

#[test]
#[ignore = "downloads twelve real USGS tiles; optional ONEKEE_TERRAIN_BENCH_LEGACY=1 bounds whole jobs to four"]
fn benchmark_live_threedep_pipeline() {
    let scratch = Scratch::new("live-pipeline-bench");
    let cache = scratch.0.join("cache.sqlite");
    super::super::db::open_cache_db(&cache).unwrap();
    let old_jobs = super::super::work_slots::Slots::new(4);
    let legacy = std::env::var("ONEKEE_TERRAIN_BENCH_LEGACY").is_ok_and(|v| v == "1");
    let spec = super::super::zoom::spec_for_zoom(50.0);
    let tiles = tile_count(12);
    let columns = if tiles > 12 { 5 } else { 4 };
    let resources = resources::Sampler::start();
    let start = Instant::now();
    std::thread::scope(|scope| {
        for index in 0..tiles {
            let old_job = legacy.then(|| old_jobs.acquire_until(|| false).unwrap());
            let admission = loop {
                if let Some(permit) = super::super::work_slots::remote_downloads().try_acquire() {
                    break permit;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            let root = &scratch.0;
            let cache = &cache;
            scope.spawn(move || {
                let _old_job = old_job;
                let tile = TileKey {
                    zoom_bucket: spec.zoom_bucket,
                    lat_bucket: (index / columns) as i32,
                    lon_bucket: (index % columns) as i32,
                };
                let bounds = GeoBounds::around(
                    crate::model::GeoPoint {
                        lat: 40.020 + (index / columns) as f32 * 0.00666,
                        lon: -105.290 + (index % columns) as f32 * 0.00666,
                    },
                    spec.half_extent_deg,
                );
                assert!(
                    build_threedep_contours(root, cache, tile, bounds, spec, admission).is_some()
                );
            });
        }
    });
    let elapsed = start.elapsed();
    let peak = resources.finish();
    let db = super::super::db::open_cache_db_read_only(&cache).unwrap();
    let completed: i64 = db
        .query_row("SELECT count(*) FROM contour_tile_manifest", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(completed, tiles as i64);
    eprintln!(
        "PIPELINE legacy={legacy} tiles={tiles} wall_ms={:.1} peak_rss_mib={:.1} peak_gdal={}",
        elapsed.as_secs_f64() * 1000.0,
        peak.rss_kib as f64 / 1024.0,
        peak.gdal_processes
    );
}
