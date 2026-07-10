# contour_pass.rs

## Purpose

Renders globe contour polylines as cached, GPU-instanced line segments. It
keeps per-frame work to uniform updates and an instanced draw while geometry is
rebuilt only when source contours or their palette changes.

## Components

### `ContourCallback`

- **Does**: Carries a layer's transform, fade, scale, and instance version into
  the egui-wgpu callback.
- **Interacts with**: `globe_scene/geography.rs`, `contour_lines.wgsl`, and
  `ContourPassResources`.

### `contour_stroke_half_px`

- **Does**: Converts the shared multiplier into a physical-pixel half-width.
  At `1×` it yields the legacy 1.15 logical-point full stroke.
- **Interacts with**: `ContourUniforms::stroke_half_px`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Globe geography | Changing thickness updates only uniforms, never contour instance caches | Moving scale into the instance version or baked geometry |
| WGSL shader | `stroke_half_px` remains a physical-pixel half-width in the unchanged uniform layout | Reordering or resizing `ContourUniforms` without matching WGSL |

## Notes

- Scale is validated by `AppModel` before reaching this module.
- The GPU default-width test protects visual parity with the pre-slider path.
