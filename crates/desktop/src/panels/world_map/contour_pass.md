# contour_pass.rs

## Purpose

Renders globe contour polylines as cached, GPU-instanced line segments. It
keeps per-frame work to uniform updates and an instanced draw while geometry is
rebuilt only when source contours or their palette changes.

## Components

### `ContourCallback`

- **Does**: Carries a layer's transform, fade, physical stroke width, and instance version into
  the egui-wgpu callback.
- **Interacts with**: `globe_scene/geography.rs`, `contour_lines.wgsl`, and
  `ContourPassResources`.

### `instances_for`

- **Does**: Keeps at most one full background instance rebuild active per
  contour layer, then schedules the latest tile-set version on a later repaint.
  A caught worker panic releases the gate while retaining the last good
  instance set for display.
- **Interacts with**: `contour_asset.rs` merged `Arc` identities and Rayon.

### `contour_stroke_half_px`

- **Does**: Converts the already-selected physical full width into the GPU
  half-width uniform.
- **Interacts with**: `ContourUniforms::stroke_half_px`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Globe geography | Changing physical width updates only uniforms, never contour instance caches | Moving width into the instance version or baked geometry |
| Globe contour cache | A stable unchanged contour Arc is a cache hit; changing tiles does not fan out simultaneous full rebuilds | Starting a rebuild for every intermediate version while one is active |
| WGSL shader | `stroke_half_px` remains a physical-pixel half-width in the unchanged uniform layout | Reordering or resizing `ContourUniforms` without matching WGSL |

## Notes

- Pixel width is normalized by `AppModel` before reaching this module.
- The GPU width test protects the one-pixel minimum's direct half-width mapping.
- Instance worker threads are named so a future native failure identifies the
  relevant work category instead of reporting only an unknown thread.
- A failed instance build is recoverable: it leaves stale geometry visible,
  clears the in-flight marker, and asks the next repaint to retry.
