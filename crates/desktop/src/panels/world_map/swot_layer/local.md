# local.rs

## Purpose
Draw a bounded native-grid gravity window in Local Earth view.

## Contracts
A padded pixel request is sampled off-thread. Previously loaded windows stay
visible during nearby requests, using their own returned bounds. North-up edges map to the
same unwrapped geographic edges in an exact clipped sea-level quad, using the existing
local projector and view extents. Longitude wrapping happens only in the grid
reader so dateline images remain continuous around the focus. The affine projector clips against the canvas before geometry is built. It does not supply geometry or marker elevations.

Projection is delegated to the shared `raster_mesh` implementation, also used
by NCEI anomaly maps. Cache, viewport selection and loading remain SWOT-owned.
