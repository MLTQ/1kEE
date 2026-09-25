# contour_fallback_tests.rs

## Purpose
Regression coverage for the deep-zoom blackout and for preserving offline fine
tiles without relying on live USGS coverage or the user's data drive.

## Contracts
Unit fixtures distinguish absent tiles from decoded empty tiles and preserve
source progress/geometry identity. The ignored integration test runs separately
because the public loader owns global caches. It creates nine SRTM tiles in a
temporary Derived directory at a Sydney focus, starts directly at maximum zoom,
checks all 150 short paths per tile survive, then adds cached fine tiles and
waits for the asynchronous upgrade. No network access is required. Missing
real source data must not silently skip the test.

Run the integration test with `cargo test -p one-thousand-electric-eye-desktop
cold_deep_zoom_reads_base_then_upgrades_to_cached_fine_tiles -- --ignored
--test-threads=1`. The normal unit suite excludes it to avoid global-cache
interference with other scene tests.
