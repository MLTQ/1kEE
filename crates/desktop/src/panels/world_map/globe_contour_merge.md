# globe_contour_merge.rs

## Purpose
Compose Earth globe contours once per changed tile snapshot, off the paint
thread. Mixed full-footprint legacy tiles and newer core tiles draw each region
once instead of accumulating opacity wherever old footprints overlap.

## Components
- `ownership` assigns each decoded tile its native core, including empty tiles.
  Missing neighboring cores may use a legacy halo. Nearest-source ordering and
  rectangle subtraction partition halos without throwing away sparse coverage.
- `compose_visible` clips complete polylines with intersection endpoints to
  visible core regions; outgoing zoom geometry survives only in visible gaps
  outside incoming coverage. It never joins across gaps. Its output replaces
  both the rendered Arc and the fallback Arc, releasing old travel history.
- `render` coalesces revisions into one worker and retains the previous Arc.
  `finish` rejects reset/root/zoom epochs and obsolete viewport revisions but
  allows coherent intermediate tile arrivals, preventing publication starvation.

## Contracts
SQLite and packed archives feed the same decoded geometry. Nothing on disk is
rewritten. Globe reads preserve complete paths before partitioning so the old
wide-tile feature cap cannot erase contours in a small retained core. Bounds,
clipping and merged-vector copies run on the worker. Unchanged frames reuse the
same Arc and GPU instance set; only one composition worker may run per cache.
