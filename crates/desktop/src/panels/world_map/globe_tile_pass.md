# globe_tile_pass.rs

## Purpose
Draw full-resolution Earth globe contours without rebuilding or uploading the
entire terrain layer on each arrival. Reuses the common globe line pipeline.

## Components
- One CPU worker reuses unchanged tile/palette instances by pinned Arc identity.
  Each point's sphere transform is reused by the next segment in its path.
- Half-degree spatial bins contain unchanged segments; conservative bounds
  include actual endpoints, including long legacy segments. Chunks are <1 MiB.
- Uploads share the globe 4 MiB / 2 ms frame budget. A complete candidate replaces the
  displayed generation atomically; no partial LOD can overlap the previous one.
- Paint rejects offscreen chunk bounds before issuing GPU work. Camera changes
  only uniforms and culling, never source geometry or instance identity.
- CPU/GPU lifecycle releases hidden terrain and rejects retired worker results.

## Contracts
No simplification or coordinate quantization. Segment endpoints and colours
match the common globe pass. `globe_contour_density` selects whole elevation
planes for wide views using one shared spacing; full geometry stays resident. Bounds culling is conservative.
Source Arcs remain pinned while their pointer-derived versions can be used.
Candidate, displayed, and active image-refinement GPU generations survive staging.

Spatial batches retain elevation spans so density changes only draw ranges,
with no instance rebuilds or uploads. Finer planes return as visible work falls.

An in-progress upload finishes its snapshot before accepting newer arrivals,
so a fast streaming reader cannot indefinitely postpone the first complete
picture. Active image-refinement versions also stay pinned until their frozen job finishes.

`globe_terrain_texture` retains full-detail coverage during movement using
spherical image reprojection. Full native snapshots refine in bounded batches
even during continuous motion. Preview density only applies outside retained
image coverage and on initial loading.

An already displayed unchanged candidate returns immediately from staging;
steady frames do not sort or walk pending uploads.

Instance publication frees replaced generations after releasing the UI cache
lock. Completed refinement jobs promptly release their extra GPU versions.
