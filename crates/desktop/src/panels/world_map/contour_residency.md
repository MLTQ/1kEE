# contour_residency.rs

## Purpose
Keep local contour geometry resident only while it can intersect the viewport.
Cache files remain unchanged; small bounds records avoid repeat offscreen reads.

## Components
- `Bounds::from_contours` scans decoded geometry on the reader worker.
- `Viewport` projects a 3D bounds box using the renderer's affine transform.
  Separating-axis tests account for yaw, pitch, elevation and edge crossings.
- `Residency` combines the current source window with screen intersection;
  unchanged source windows skip redundant pruning.
- `set_local_viewport` updates both Earth tiers and releases inactive bodies.
- `prune` drops decoded tiles and progress, invalidating the background merge.

## Contracts
- Unknown tiles use the legacy footprint and conservative planetary heights;
  decoded tiles use their actual geometry bounds. Empty tiles retain core coverage.
- Eight logical pixels of edge margin cover stroke feathers and roundoff.
- No geometry scan or disk I/O runs during paint. Metadata retention is bounded.
- Late readers record bounds but cannot publish offscreen geometry.
- Previous CPU merge snapshots survive only until their replacement completes;
  an empty view releases its merge immediately.
- Source-detail fallback distinguishes culled cells from unavailable cells.

## Validation
`contour_residency_tests.rs` covers oblique visibility, elevation, crossing lines,
pan eviction, late readers, metadata reuse, and reloading when a tile returns.
