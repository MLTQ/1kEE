# contour_read_tests.rs

## Purpose
Checks that streaming contour reads produce exactly the same geometry and
ordering as the prior SQLite blob-sort implementation, and measures real data.

## Components
- Synthetic MultiLineString fixture: covers positive/negative elevation ties,
  fid order, multiple parts, simplification, and length-budget truncation.
- `baseline`: test-only reproduction of the prior read/decode algorithm.
- `benchmark_dense_cached_tile`: ignored, read-only benchmark selected with
  `ONEKEE_CONTOUR_BENCH_DB`. Warms up first, reports three pairs of timings,
  and requires identical geometry/order on every comparison.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `contour_asset.rs` | Test child can exercise private read/parse helpers | Changing ordering or simplification semantics |
| Developer benchmark | Explicit opt-in; no writes to the supplied cache | Mutating operator data or requiring fixtures in the normal suite |

- The row-progress regression reads a real 300-row temporary SQLite tile and
  checks intermediate/final counts against its manifest, including empty blobs.

- `benchmark_cached_reader_concurrency` compares eight distinct dense tiles
  with 1/2/4 readers, repeats 1/2 to expose cache warmth, and requires identical
  per-tile hashes of all selected geometry/order. It only reads the real cache.

- Streaming regressions verify early tile publication while the batch remains
  in flight, decoded-versus-rendered state, stale epoch rejection after reset,
  old completion not clearing a new batch, and cancellation between tiles.

- Manifest publication retains the selection-start revision, ensuring a queue
  wakeup during an in-flight selection cannot be consumed by stale results.
