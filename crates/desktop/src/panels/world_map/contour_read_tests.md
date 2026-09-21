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
