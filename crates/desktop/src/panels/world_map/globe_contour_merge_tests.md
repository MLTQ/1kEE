# globe_contour_merge_tests.rs

## Purpose
Regress globe brightness seams without rebuilding user data. Synthetic legacy
footprints cross multiple cells so the tests detect both double drawing and
loss of useful sparse coverage outside a source's own core.

## Coverage
- Legacy/legacy overlaps draw each sampled location once, keeping outer coverage.
- Modern and decoded-empty cores override old halos; clipped gaps never bridge.
- Missing replacement cells retain outgoing zoom geometry.
- Arrivals retain the published Arc and cannot starve intermediate publication;
  resets reject late workers without opening a second worker slot.
- Two hundred successive camera stops feed the previous picture into the next
  composition and verify that old Arcs are freed instead of accumulating in
  fallback history. Stale viewport workers cannot restore evicted geometry.
- Resident sources far outside the current request window still compose.
  Outgoing offscreen LOD survives below budget; pressure removes it while
  preserving visible fallback paths.
- Opt-in `real_moscow_mixed_cache_coverage_is_disjoint` reads three installed
  Hilbert tiles (old LineString and new MultiLineString), checks disjoint region
  ownership, and proves southern legacy coverage survives. Read-only throughout.

The same regressions now inspect tile frames rather than flattened runtime
vectors. Added explicit Arc reuse after neighboring arrivals and root-transition
fallback release. Budget trimming acts on offscreen fallback pieces, not points.
