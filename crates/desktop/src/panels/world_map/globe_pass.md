# globe_pass.rs

## Purpose
Render the globe backdrop and graticule in one native GPU callback. Per-frame
uniforms supply the camera, active body and current semantic theme colors.

## Contracts
- `color_to_linear` is for linear-light shading and administrative fill input;
  it does not imply an sRGB framebuffer. Alpha is unchanged.
- The globe shader encodes final colors according to the actual target format
  through `gpu_color`, matching egui's preferred Unorm surface.
- Uniform layout and perspective remain synchronized with `globe.wgsl`.
- Source generation, contours and terrain tile residency are independent.

The 128-byte uniform block now uses former padding at byte 120 for the Atlas
flag, with one remaining padding word. It disables synthetic Earth geography
and dim exposure in Atlas; measured relief paints above the plain ocean base.

`GLOBE_WGSL` composes shared `globe_surface.wgsl` with the desktop entrypoint.
Shader regression tests consume this exact production assembly so extraction
for the OpenXR renderer cannot leave tests validating an incomplete source.
