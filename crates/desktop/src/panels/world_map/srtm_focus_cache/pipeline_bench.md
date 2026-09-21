# pipeline_bench.rs

## Purpose
Opt-in end-to-end terrain measurements against real GDAL, SQLite, and USGS.
Outputs live in unique disposable directories and are removed by RAII;
no benchmark writes to or replaces the operator's existing contour cache.

## Components
- `benchmark_contour_processing_concurrency`: contours/imports eight copies
  of the same 2400-pixel raster at 1, 2, 4, and 6 workers, including repeated
  2/4-worker rounds. Fresh shared databases expose writer contention. Requires
  identical row/byte counts and eight committed manifests in each round.
- `benchmark_live_threedep_pipeline`: twelve adjacent real tiles. Set
  `ONEKEE_TERRAIN_BENCH_LEGACY=1` and `ONEKEE_TERRAIN_WORKERS=2` to reproduce
  the prior whole-job limit, then rerun without the legacy setting.
- `Scratch`: removes only the unique benchmark output directory on drop.
  Defaults to system temp; `ONEKEE_TERRAIN_BENCH_OUTPUT` selects a different
  parent volume. Processing rounds delete each completed database so storage
  use stays bounded to one round plus active GeoPackages.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| Normal test suite | Both tests ignored, no network or GDAL required | Removing opt-in |
| Supplied raster | `ONEKEE_TERRAIN_BENCH_RASTER` input is read-only | Mutating source fixtures |

## Running
Use release mode, `ONEKEE_TERRAIN_TIMINGS=1`, `--ignored --nocapture`, and
`--test-threads=1`. The processing benchmark fetches a single USGS raster if
no existing raster path is supplied. The pipeline benchmark always uses USGS.
Network/cache warmth varies; compare repeated runs, not timing assertions.
These headless measurements do not establish interactive frame rates.

- `ONEKEE_TERRAIN_BENCH_TILES=1..25` sizes either batch (defaults remain 8/12).
  `ONEKEE_TERRAIN_BENCH_WORKERS=4,8,25,4` selects processing sweep sizes;
  `ONEKEE_TERRAIN_DOWNLOADS` varies the live pipeline download limit. The live
  25-tile case uses a 5x5 window. `bench_resources.rs` samples process-tree RSS
  and concurrent GDAL processes, excluding unrelated applications.
- Scratch creation fails rather than reusing a pre-existing directory.
- Full-view results, including repeated warm network runs and memory caveats,
  are recorded in `docs/terrain-pipeline.md`.
