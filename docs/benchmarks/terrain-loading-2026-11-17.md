# Terrain loading measurements, 17 November 2026

## Local contour tiles

The release `benchmark_local_core_load` test opens a private SQLite snapshot,
reads the tile, decodes its geometry, clips to its owned Earth core and returns
the ordered contour paths. The timer stops before checksumming. Four processes per version in alternating order ran on the same CPU core on this machine. The harness
requested Linux file-page eviction for the two fixture files before each first
read. Each process then repeated the read five times. This measures already-built
local tiles. Source download, contour generation and rendering have separate costs.

| Fixture | Ordered parts | Vertices | Before first / warm | After first / warm |
| --- | ---: | ---: | ---: | ---: |
| Native Earth 4/152/-435 | 45,828 | 1,324,808 | 69.52 / 39.89 ms | 26.12 / 12.14 ms |
| Hosted Earth 8/1242/-4060 | 7,712 | 301,562 | 15.22 / 10.72 ms | 10.31 / 6.68 ms |

Medians use four first reads and twenty repeated reads per fixture and version.
The two ordered geometry hashes were `74aa18f05912e6ca` and `f2616bfc05bf824a`
respectively in both versions. The normal renderer can reuse a resident tile
without issuing another SQLite read. To reproduce the read timing, set
`ONEKEE_LOCAL_TILE_DB` to a private snapshot and run the ignored test.

## Projected LiDAR

On the complete cached Sharon sea cliff survey, loading the original LAZ and
transforming its XY coordinates fell from 65.97 s to 10.97 s. Both runs decoded
13,268,620 returns, retained 377,003 inside the 1 km target, and transferred zero
bytes. The sample uses EPSG:2039 and the original vertical values. Run
`target_live_israel_original_points_and_retained_dem` with
`ONEKEE_TARGET_TEST_CATALOG` set to a catalog with the local survey.

## Globe terrain

The native Yemen/Red Sea fixture has 46 tiles and 39,491,374 segments. On the
RTX 4090 (Vulkan 615.78.08) at a 4 MiB byte allowance and 2 ms admission
allowance, the worst map-thread staging call went from 221.11 ms to 0.13 ms.
With the worker, staging p99 was 0.024 ms. The complete first geometry snapshot
took 0.89 s with a 1 ms polling interval. This tight polling run measures CPU
admission and publication. Driver allocation still consumes worker time.

A separate 1920 × 1080 offscreen route uses real source tiles and the production
egui contour callback. It pans, zooms and returns with a 60 Hz target. Frame
durations include egui, rendering, queue submission and a GPU wait. They exclude
the pacing sleep, the desktop compositor and unrelated application layers.

| Leg | Tiles | Frame work p95 | Frame work p99 | Time until that line set was complete |
| --- | ---: | ---: | ---: | ---: |
| Arrive | 18 | 9.86 ms | 11.25 ms | 1.87 s |
| Pan and zoom | 24 | 14.83 ms | 17.44 ms | 2.01 s |
| Return | 18 | 11.25 ms | 16.03 ms | 1.40 s |

That route reused 29 retired buffers. Both Globe measurements use
`benchmark_native_globe_tiles` with `ONEKEE_NATIVE_BENCH_DB` pointing to the
native database and `ONEKEE_GPU='RTX 4090'`. Add `ONEKEE_TERRAIN_ROUTE=1`
for the paced route. These are single-machine measurements.

## Offline verification

A 5° Rocky Mountains pin built from local GEBCO occupied 11,522,048 bytes.
Local and Globe returned identical height arrays from the installed file, with
a sampled range of 2,002 to 4,148 m. Separate native and hosted contour packages
verified, installed twice, and rejected a damaged copy while preserving the
prior destination records.
