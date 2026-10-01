# composition.rs

## Purpose
Compose ready detail levels into disjoint geographic regions without waiting
for every tile in a replacement grid. Missing sources leave old terrain visible.

## Components
- `compose` subtracts higher-priority ready cores from older cores, then clips
  only affected geometry. Empty decoded cells also own their core; unavailable
  cells never enter the composition. New and old contours cannot double-draw.
- `Piece` reuses a tile's clipped Arc while its original geometry and residual
  regions are unchanged. Unclipped modern tiles retain their original Arc.
- `clip_contours` uses the archive's tested line clipper: insert intersections,
  split at exits, preserve disconnected paths, and assign shared boundary edges.
- `visible_bounds` culls the owned core and its elevation range.
- The globe merge reuses `clip_contours` and `subtract` with its own ownership
  regions, preserving useful legacy geometry beyond missing neighboring cores.

## Contracts
Runs on one coalesced worker, never during paint. Source grids and files do not
change. Legacy overlapping geometry is clipped in memory. The result includes
a matching CPU merge and stable per-tile GPU inputs, used for both zoom directions.
