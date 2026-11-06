# globe.wgsl

## Purpose
Shade the globe backdrop, body-specific terrain and graticule from per-pixel
ray/sphere intersections. Color mixing and lighting use linear inputs.

## Contracts
- Camera/uniform layout matches `GlobeUniforms` in `globe_pass.rs`.
- Opaque sphere and translucent halo outputs pass through `map_output_linear`.
  Unorm framebuffers need display encoding; sRGB framebuffers supply it themselves.
- The shared color helper is prepended by `gpu_color::shader`.
