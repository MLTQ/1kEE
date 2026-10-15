# contour_fallback_tests.rs

## Purpose
Regression coverage for the deep-zoom blackout and for preserving offline fine
tiles without relying on live USGS coverage or the user's data drive.

## Contracts
Unit fixtures distinguish absent tiles from decoded empty tiles and preserve
source progress/geometry identity. The ignored integration test runs separately
because the public loader owns global caches. It creates nine SRTM tiles in a
temporary Derived directory at a Sydney focus, starts directly at maximum zoom,
checks all 150 short paths per tile survive, then adds one cached fine tile
and verifies it appears alongside base coverage. It completes the fine window,
waits for the asynchronous upgrade, then pans into a missing edge and verifies
the overlapping fine tile survives the fallback. No network access is required. Missing
real source data must not silently skip the test.
SRTM fixtures use the tagged native cache; hosted fixtures use the existing
separate cache path, exercising the same source routing as production.

Run the integration test with `cargo test -p one-thousand-electric-eye-desktop
cold_deep_zoom_reads_base_then_upgrades_to_cached_fine_tiles -- --ignored
--test-threads=1`. The normal unit suite excludes it to avoid global-cache
interference with other scene tests.

The fine-tier coverage test also distinguishes deliberately culled cells from
missing visible tiles, so residency pruning cannot force permanent base fallback.

Fixtures include the optional reader-computed tile bounds carried to the LOD
handoff. Missing fixture bounds use the tile core for coverage.

Partial fine coverage must be passed alongside base terrain with stable Arcs
and the requested zoom, including decoded empty cells. Unpublished fine tiles
are excluded. The base retains its own progress-grid coordinates. A separate
handoff regression checks that this contract preserves fine cores during a pan
and still permits a deliberate zoom-out.
