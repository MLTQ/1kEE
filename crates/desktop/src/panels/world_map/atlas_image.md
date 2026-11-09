# atlas_image.rs

## Purpose
Convert measured elevation/depth into unlit Atlas pigments. Lighting belongs to
the runtime shader, so terrain never carries fixed shadows when the light moves.

## Components
- `image_for`: validate dimensions/geographic bounds, then generate pigment rows
  in parallel. Heights remain unchanged for the actual 3D geometry.
- `elevation_color`: pale parchment plains, ochre foothills, oxide/vermilion reds
  through the mountain elevations, pale summits and deep petrol-blue bathymetry.

## Contracts
Missing/nonfinite/out-of-range heights are transparent. A neighbor, camera,
source resolution or light direction cannot change a pixel's base pigment.
Colors represent elevation/depth, never a fabricated geological classification.
