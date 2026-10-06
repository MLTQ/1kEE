# globe_terrain_texture_tests.rs

## Purpose
Exercise the stationary native terrain image cache on the actual GPU.

## Coverage
A 2.2 million segment fixture starts with an intentionally omitted preview
plane. Refinement spans multiple frames, restores that plane only when complete,
then reuses identical pixels with no cursor changes. Camera changes invalidate
the image and restart refinement. All targets are private offscreen textures.

The restored image must match the RGB pixels from a one-pass draw of every
original segment. Its reference fixture is independently checked nonempty.
