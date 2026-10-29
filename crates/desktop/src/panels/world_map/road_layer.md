# road_layer.rs

## Purpose
Loads and elevates OSM roads in one background job, then submits retained GPU
line batches. No per-frame CPU road projection/tessellation or point cutoff.

## Contracts
- Both classes share a viewport-plus-margin cache. Visibility toggles only
  filter batches and keep the last loaded region, even with both classes off.
- Root, data generation, theme or escaped coverage triggers a single worker.
  Old geometry remains displayable while refreshing within the same root.
- Workers build geography/elevation instances through `road_geometry`; frame
  snapshots clone Arcs under a short, nonblocking lock. Retired cache disposal
  happens outside the lock and explicit resets defer disposal off the UI thread.
- Explicit invalidation increments an epoch, preventing in-flight stale builds
  from resurrecting reset data. Failed/panicked workers release the build gate.
- Subdued roadbed colors come from the active theme. Major/minor widths are
  1.1/0.6 logical points; the GPU boundary converts them to physical pixels.
  Minor batches draw first so highways remain continuous at intersections.
  Projection is shared with terrain/markers.
- `road_cache_building_bounds` continues to describe the pending load envelope.
- Existing per-way 192-vertex simplification remains; missing source geometry
  cannot be reconstructed by this renderer.
