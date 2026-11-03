# render.rs

## Purpose
Draw one prepared anomaly image using the shared geographic raster projector.

## Components
- `draw_globe` caches one immutable mesh keyed by texture and camera/layout.
- `draw_local` uses the clipped affine sea-level projector to keep a whole
  regional image visible at deep zoom, with source UVs and dateline alignment.
- `clear` releases the thread-local globe mesh when the owning cache resets.

## Contracts
Images load through the single-flight `texture` function. Failed/loading images
produce no geometry; incompatible product/drive images never substitute. The
raster never changes elevation and is below contours/markers in both scenes.
