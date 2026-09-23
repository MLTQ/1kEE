# road_geometry.rs

## Purpose
Builds road GPU instances on a background worker. Replaces the length-prioritized
400k/800k point cutoffs with bounded upload batches that retain every loaded way.

## Contracts
- Existing 192-vertex-per-way thinning, endpoint retention, aligned baked heights
  and 3 m display offset remain unchanged; no whole-way budget or sorting.
- Major/minor classification, colours and widths retain existing semantics.
- Each batch holds at most 65,536 segments (2 MiB). Splitting operates on edges,
  retaining joins across batches and never connecting separate OSM ways.
- Non-finite segments are skipped independently; no bridge across invalid points.
- Instances retain original geography/elevations so camera motion never rebuilds
  or reprojects the CPU data. Colours are converted once per layer.
- `RoadGeometry` owns immutable Arc-backed batches for cheap frame snapshots.
- Regression/optional read-only regional benchmark lives in `road_geometry_tests`.
