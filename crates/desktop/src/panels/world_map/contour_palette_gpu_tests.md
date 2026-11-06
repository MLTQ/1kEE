# contour_palette_gpu_tests.rs

## Purpose
Compare the real native contour shader with egui's independent line renderer.
This catches the palette/opacity flash when tile drawing moves from CPU to GPU.

## Contracts
- Offscreen 64×64 targets exercise both Unorm and sRGB framebuffer formats.
- Readback compares original Topo orange, translucent blue, a pass fade and two
  other non-primary theme colors, with baked colors and live palette uniforms.
- CPU and GPU lines are disjoint and sampled in their fully covered cores.
- Both target formats also compile the actual linear-light globe backdrop
  pipeline, which uses the other shared output conversion entry point.
- The ignored hardware test creates no app window, network requests or settings.
