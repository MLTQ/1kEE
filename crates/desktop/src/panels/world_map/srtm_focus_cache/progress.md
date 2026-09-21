# progress.rs

## Purpose
Tracks actual per-tile build work for the loading grid, independently of time.
The UI reads atomic handles from its background manifest snapshot.

## Components
- `BuildProgress`: four equally weighted workflow stages. Source download or
  raster preparation is 0–25%, contour generation 25–50%, committed cache import
  50–75%; the loader owns decoding and publication from 75–100%.
- Byte/row callbacks advance only with a known positive denominator. The final
  unit in a stage requires its successful completion. GDAL stages without a
  count hold still until the subprocess succeeds; these are not time estimates.
- Registry: weak handles keyed by database path, zoom, latitude and longitude.
  New attempts replace previous handles. Expired handles are pruned on start.
- `snapshot`: background-only root/zoom selection; old workers cannot update
  a replacement attempt's handle. `clear` detaches workers on manual resets.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| GDAL and imports | No progress for unvalidated responses or uncommitted completion | Reporting 100% before publication |
| Local manifest | Handles identify the same database and zoom as its assets | Mixing roots or polling SQLite from paint |
| Grid | Fractions measure completed workflow work and never elapsed time | Timed extrapolation or cyclic reset |

Workers pass their own handle into later stages; a registry replacement never
redirects an old worker's row/commit updates into the new attempt.
