# globe_terrain_texture_tests.rs

## Purpose
Exercise retained native detail and rolling refinement on the actual GPU.

## Coverage
A 2.2-million-segment fixture uses an intentionally absent 200m preview plane.
It requires multiple refinement frames, then matches every RGB pixel from a
single full-resolution reference draw. During continuous camera movement,
readback must remain nonempty and its centroid must follow the newly projected
reference within 1.5 pixels. Frozen jobs must finish even while movement
continues. After settling, composited pixels again match the reference exactly.
All targets are private offscreen textures, with no app/settings changes.

`resident_theme_changes_recolor_complete_and_inflight_globe_images` uses dense
red/green class coverage plus a minor contour. It switches palettes and back on
the same resident buffers and completed texture, verifies changed pixels and exact
round-trip output, then switches during a two-million-segment refinement step.
The frozen job finishes on its original target; no geometry upload or image
retirement occurs because only colors changed.

The same resident-image regression compares the actual Topo orange and faded
blue with independently rendered class coverage. Unlike primary RGB fixtures,
these midtone channels expose an incorrect color-space conversion.
