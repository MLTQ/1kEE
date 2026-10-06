# globe_terrain_texture.rs

## Purpose
Reuse the terrain image while the globe is stationary. Pips, UI and OBS frames
no longer require another draw of every native contour segment.

## Components
- A changing camera/frame/style draws a responsive whole-plane preview.
- Stable frames refine all native elevation planes into a second transparent
  texture, at most two million segments per frame. A completed image replaces
  the preview atomically; partial images and different LODs never overlap.
- Subsequent unchanged frames composite one textured quad. The full-detail image
  remains until source, camera, viewport, palette, width or fade changes.
- Texture dimensions follow the physical viewport. Blending remains premultiplied
  and the render target matches the window format, including sRGB conversion.

## Contracts
All original segments and planes return after settling, without a large single
GPU draw. Refinement uses unchanged geometry and deterministic draw order.
Textures and their pinned source frame are owned by the native GPU layer and
released on hiding/reset. Offscreen batches are culled before refinement work.
