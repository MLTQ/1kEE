# tree_layer.rs

## Purpose
Loads cached polygons on a background worker and projects them into the local
terrain view. Uses packed archive/binary cells through the shared cell loader.

## Contracts
- Valid baked vertex heights are reused; only missing/invalid heights sample
  runtime SRTM. The existing display offset and polygon flags remain unchanged.
- Full point order/detail are retained. Cache invalidation follows root, bounds,
  and OSM data generation. No new disk work runs in the paint loop.

The worker prepares Lyon concave-polygon triangles with interpolated source
heights through `tree_geometry`. The painter reprojects those triangles and uses
open-path boundary strokes, avoiding convex fan fill and unbounded closed miters.
Projection keeps offscreen vertices so GPU clipping preserves crossing geometry;
invalid points break outlines instead of joining unrelated survivors. Open source
rings remain outlines. A late cache from a different root cannot be painted.

In Atlas, the scene uses `atlas_forest` coverage on the terrain itself instead of this elevated vector path. This path remains the corrected renderer for other map themes.
