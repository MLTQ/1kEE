# color_output.wgsl

## Purpose
Shared output transfer functions for native map shaders, selected by the actual
framebuffer format through the `SRGB_FRAMEBUFFER` pipeline constant.

## Contracts
- Display-space lines call `map_output_gamma` after coverage and pass fading.
- Linear-light globe shading calls `map_output_linear` at its final boundary.
- Retained same-format terrain textures call `map_sample_gamma` before palette
  interpretation; automatic texture decoding must not change class coverage.
- All helpers preserve alpha. No alpha gamma correction or arbitrary power is used.
- Shader assembly prepends this file once; no uniform layouts change.
