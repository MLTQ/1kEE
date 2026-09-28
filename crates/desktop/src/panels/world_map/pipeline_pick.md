# pipeline_pick.rs

Builds a camera-independent bounding-volume hierarchy of the exact unit-sphere
segments uploaded for public pipelines. Built once on the same worker as the
GPU batch, with an owner index for every segment. No metadata duplication.

- Median splits of segment centers on the longest box axis, 16-entry leaves.
- Queries project conservative box corners then only candidate leaf segments.
  A box crossing the near plane stays eligible; exact leaf tests decide hits.
- Uses the shader's yaw/pitch, perspective, 1/distance horizon and near-plane
  rejection. Both endpoints must be visible, matching whole-segment GPU culling.
- Hit distance follows the straight projected segment, including coarse routes
  and date-line segments; never assumes a great-circle surface path.
- Query returns feature indices and logical-pixel distances; shared hover logic
  deduplicates fragments and handles overlaps. Hidden/filter-excluded lines
  cannot be picked because the index owns the same filtered batch as rendering.
- Camera changes require no tree rebuild or full-scene CPU projection. The
  return count measures exact segment tests for the real-data performance check.
