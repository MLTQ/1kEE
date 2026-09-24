# contour_grid.rs

## Purpose
Assign each Earth contour cache address one nonoverlapping core while preserving
existing zoom/latitude/longitude keys and approximately the old sample spacing.

## Components
- CoreTile::new derives identical f64 shared edges and a two-pixel source halo.
- clip_line clips segments, preserves boundary crossings and separates parts.
- Bounds is the shared f64 raster/ownership rectangle.

## Contracts
- Grid step is historical f32 half_extent * 0.45, promoted before multiplying.
- Source pixels shrink from N to ceil(0.225*N)+4. Only the core is persisted.
- North/east boundary-aligned edges belong to the neighbor; crossing endpoints
  remain in both tiles. No geometry connects across an excursion outside.
- Old full-footprint entries stay readable and are not deleted or clipped during
  lookup. Shrinking them requires a separate coverage-preserving migration.
