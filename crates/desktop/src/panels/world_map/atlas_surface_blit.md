# atlas_surface_blit.wgsl

Composite the private depth-tested terrain color image into egui's depth-free
UI pass. Stored colors are display-space RGBA; the shared output helper adapts
to Unorm and sRGB framebuffers. Transparent pixels preserve the scene background.
