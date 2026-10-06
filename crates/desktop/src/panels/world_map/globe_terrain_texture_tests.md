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
