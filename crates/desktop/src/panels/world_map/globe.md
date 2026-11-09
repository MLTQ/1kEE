# globe.wgsl

## Purpose
Build the desktop camera rays and intersect the globe in screen space. The desktop and VR passes share globe surface lighting, body-specific terrain and graticules in `globe_surface.wgsl`.

## Contracts
- Camera/uniform layout matches `GlobeUniforms` in `globe_pass.rs`.
- Opaque sphere and translucent halo outputs pass through `map_output_linear`.
  Unorm framebuffers need display encoding; sRGB framebuffers supply it themselves.
- The shared color helper is prepended by `gpu_color::shader`.
- `BACKDROP_EXPOSURE` applies 0.12× linear-light gain to the shaded sphere and
  halo across every theme/body. Contour colors and independent overlays retain
  their existing output conversion and brightness.
- Rim brightening uses ascending smoothstep edges over the final 8% of the
  view-angle rim factor. It must not wash the whole visible surface with the
  brighter wireframe color, as the old reversed transition did.
