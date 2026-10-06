# globe_tile_gpu_tests.rs

## Purpose
Test real GPU staging and rendering using private offscreen textures.

## Coverage
The hardware regression exceeds the per-frame upload budget, keeps the previous
picture until all replacement tiles upload, releases retired buffers, and
compares pixels from the old six-vertex pipeline against the new strip+culling.
Both pipelines consume identical native segment coordinates.

The opt-in benchmark reads installed native Yemen/Red Sea tiles from an explicit
`ONEKEE_NATIVE_BENCH_DB`, checks zero rebuilds for surviving tiles, reports bounded
upload times, then alternates old/new GPU submissions at 1920×1080. Timing includes
CPU submission and GPU wait and does not claim end-to-end application FPS.
Neither test opens an application window or modifies the source cache.

The native benchmark also measures whole-plane density selection. Pixel parity
keeps all planes enabled to isolate batching, culling and triangle topology.

Real egui callback prepare/finish ordering also verifies that hiding the layer
releases all native GPU buffers and that showing it again uploads successfully.
Optional `ONEKEE_NATIVE_BENCH_IMAGE` saves the private diagnostic render.

Benchmark output separates moving-preview cost, bounded all-plane refinement,
and steady cached full-detail compositing. Saved diagnostics show all native
planes after refinement, not the sparse interactive preview.

## Measured 2026-10-06
Apple M1 Pro / Metal, optimized test build, 1920×1080, globe zoom 24 centered
at 15°N 45°E. Read 46 actual native cached tiles: 39,491,374 segments. No app,
pips, UI, camera video, or OBS work is included in these offscreen timings.

- Previous whole-layer six-vertex drawing: median 146.08 ms.
- Interactive whole-plane preview (3,200 m spacing, 1,305,839 segments): 4.87 ms.
- Stationary full-native image compositing: median 1.23 ms.
- All-plane refinement: 21 frames including preview, worst step 13.27 ms.
- Initial spatial instance build: 335.60 ms, on a background worker.
- Tile-set update reusing 45 tiles: 0.125 ms.
- Previous single upload: 751.93 ms; staged uploads use 8 MiB per frame
  (137 steps in this cold fixture, worst observed CPU submission 25.72 ms).

Upload byte limits do not guarantee a wall-time ceiling; allocator/driver
pressure can still cause occasional stalls. These timings are not application
FPS promises. The complete desktop suite passed (260 tests); three separate
GPU regressions passed, including pixel equivalence and hidden-layer cleanup.
