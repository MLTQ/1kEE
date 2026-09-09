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

### `contour_feather_px`

- **Does**: Chooses a narrow physical-pixel anti-alias fringe for the GPU
  contour quad, scaling gently with wide strokes while keeping the 1–5 px
  range at a half-pixel fringe.
- **Interacts with**: `ContourUniforms::feather_px` and the edge coverage in
  `contour_lines.wgsl`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Globe geography | Changing physical width updates only uniforms, never contour instance caches | Moving width into the instance version or baked geometry |
| Globe contour cache | A stable unchanged contour Arc is a cache hit; changing tiles does not fan out simultaneous full rebuilds | Starting a rebuild for every intermediate version while one is active |
| WGSL shader | `stroke_half_px` remains a physical-pixel half-width and `feather_px` remains a physical-pixel edge margin in the unchanged uniform layout | Reordering or resizing `ContourUniforms` without matching WGSL |
| Any caller | A layer of any size uploads without exceeding the device buffer limit | Allocating one buffer per layer again |

## Notes

- Pixel width is normalized by `AppModel` before reaching this module.
- The GPU width test protects the one-pixel minimum's direct half-width mapping.
- The feather is intentionally narrower than the former fixed 1 px margin so
  dense global contour layers do not visually balloon at small widths.
- Three things decide how heavy a stroke actually looks, and only the first is
  the width uniform:
  1. **Width.** `stroke_half_px`, straightforward.
  2. **Fringe.** Below one pixel `contour_feather_px` is `width * 0.5`, so the
     drawn footprint is twice the selected width. A flat fringe floor made a
     quarter-pixel stroke two-thirds fringe, which is why thin settings stopped
     getting thinner. At one pixel and above the fringe is unchanged.
  3. **Sub-pixel coverage.** A stroke narrower than a pixel cannot be drawn
     narrower than a pixel; it has to be drawn fainter. The fragment shader
     scales by `clamp(2 * stroke_half_px, 0, 1)`, which is exactly 1.0 at a
     pixel and above. Without it a 0.25 px setting still painted whichever
     pixels the quad covered at full opacity — a broken one-pixel line rather
     than a fine one.
- The lengthwise feather extension is capped at half the segment length.
  A segment shorter than its own feather would otherwise inflate into a blob
  roughly `2 * feather` across, and densely sampled contours are mostly such
  segments — which is why they read as far heavier than an isolated coastline
  at the same width setting.
- Instance worker threads are named so a future native failure identifies the
  relevant work category instead of reporting only an unknown thread.
- A failed instance build is recoverable: it leaves stale geometry visible,
  clears the in-flight marker, and asks the next repaint to retry.

### `split_instance_buffers`

- **Does**: Splits an instance slice across as many vertex buffers as the
  device's `max_buffer_size` requires, and `paint` draws each in order.
- **Interacts with**: `local_contour_pass`, which reuses it for its per-tile
  uploads.
- **Rationale**: A dense layer can exceed the limit in one allocation. wgpu's
  default is 256 MiB and a global contour layer reached 287 MB, which is a hard
  `Device::create_buffer` validation panic rather than a degraded frame.
  Instances are independent, so N buffers drawn in order render exactly as one
  would.
- `main.rs` separately raises `max_buffer_size` to whatever the adapter
  reports, which reduces how often the split is needed but is not what makes it
  safe — the split has to hold on any device.

