# tree_layer.rs

## Purpose
Loads cached polygons on a background worker and projects them into the local
terrain view. Uses packed archive/binary cells through the shared cell loader.

## Contracts
- Valid baked vertex heights are reused; only missing/invalid heights sample
  runtime SRTM. The existing display offset and polygon flags remain unchanged.
- Full point order/detail are retained. Cache invalidation follows root, bounds,
  and OSM data generation. No new disk work runs in the paint loop.
