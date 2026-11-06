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
