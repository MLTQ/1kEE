# raster_mesh.rs

## Purpose
Project a north-up geographic image onto the existing globe or local sea-level
surface. SWOT VGG, NCEI anomaly imagery and imported satellite images share it.

## Components
- `globe` tessellates exact geographic bounds, maps their UV edges and omits
  cells behind the perspective horizon. Detailed images adapt tessellation to
  camera scale; global overviews retain two-degree cells.
- `local` inverts the actual clip rectangle through the affine sea-level
  projector and clips image bounds before drawing each visible longitude copy.
  It unwraps regional longitudes around the focus. Both edges of a full-width
  texture can meet in a dateline viewport. No coarse-vertex rejection can blank
  a large regional image at close zoom.

## Contracts
Bounds are west/south/east/north pixel edges in degrees; longitudes may be
unwrapped across the dateline. Images use north-up UVs. Callers own texture
lifetimes and mesh caching. SWOT regression tests pin horizon, UV and dateline
behavior of this shared implementation.

Local regression tests cover deep zoom, steep pitch, rotated views, actual
canvas coverage, UV retention, dateline placement and empty regions.
