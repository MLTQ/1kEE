# local_contour_lines.wgsl

## Purpose
Project geographic line instances through the shared local oblique camera,
expand anti-aliased physical-pixel strokes and optionally reject hidden contours.
Also supplies the depth-only contour-curtain vertex entry point.

## Contracts
- Uniforms exactly match the 128-byte Rust `LocalContourUniforms`; group 0 is
  the dynamic pass uniform, group 1 the sampled visibility depth texture.
- The screen projection matches `LocalProjectionParams::project_logical`.
- Main line clip depth remains zero because egui has no depth attachment.
  Ground distance is passed separately to the fragment shader for comparison.
- `vs_curtain` projects source contour tops and extends them downward to the
  viewport bottom at constant geographic ground distance. No fragment/color
  output, shading or height interpolation between contours is used.
- Curtain tops are inset below crests; simplified geography and pixel derivatives
  provide comparison tolerance. Clear pixels never hide any contour.
- Roads and disabled views bypass depth sampling and retain existing AA/widths.
