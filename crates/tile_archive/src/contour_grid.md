# contour_grid.rs

## Purpose
Assign each Earth contour cache address one nonoverlapping core while preserving
existing zoom/latitude/longitude keys and approximately the old sample spacing.

## Components
- CoreTile::new derives identical f64 shared edges and a two-pixel source halo.
- CoreTile::srtm preserves those owned bounds while sampling native GL1 posts
  on the same absolute one-arc-second grid at every zoom. Pixel centers are
  integer arc-seconds; outward snapping and a two-pixel halo cover each core.
- SRTM_NATIVE_DB_NAME identifies the separate native-resolution cache, so old
  coarse-raster geometry cannot masquerade as native data.
- clip_line clips segments, preserves boundary crossings and separates parts.
- Bounds is the shared f64 raster/ownership rectangle.

## Contracts
- Grid step is historical f32 half_extent * 0.45, promoted before multiplying.
- Source pixels shrink from N to ceil(0.225*N)+4. Only the core is persisted.
  This legacy rule applies to `new`; `srtm` always uses native post spacing.
- North/east boundary-aligned edges belong to the neighbor; crossing endpoints
  remain in both tiles. No geometry connects across an excursion outside.
- Old full-footprint entries stay readable and are not deleted or clipped during
  lookup. Shrinking them requires a separate coverage-preserving migration.
