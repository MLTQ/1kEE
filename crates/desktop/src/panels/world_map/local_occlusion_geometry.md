# local_occlusion_geometry.rs

## Purpose
Build compact invisible curtains directly beneath actual contour paths. There
is no inferred heightfield, midpoint interpolation, DEM fetch or visible fill.

## Contracts
- Runs on the existing contour instance worker, never during paint.
- Selects up to about 24 elevation levels including the highest; prefers longer
  paths when necessary. Omitted contours only reduce occlusion coverage.
- Radial simplification retains path endpoints and source heights. Its maximum
  geographic error is returned so the renderer can tolerate displacement.
- At most 4,096 segments per tile. Invalid coordinates break paths; no bridge
  joins disconnected paths. No-data input yields no occluder.
- Geometry shares the contour tile version and upload/residency lifecycle.

## Validation
Tests pin the count bound, exact elevations, endpoints and invalid-point gaps.
This is an intentionally conservative visibility approximation, not a terrain
surface suitable for marker heights, lighting, shadows or export.
