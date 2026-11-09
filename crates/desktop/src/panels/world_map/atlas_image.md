# atlas_image.rs

## Purpose
Shade measured elevation into a vintage relief image: parchment lowlands,
mineral mountain tints, blue bathymetry and engraved-looking terrain shadows.
Color represents height/depth; no geological classification is invented.

## Components
- `HeightGrid::image` shades a global overview; `image_for` shades exact regional
  pixel-edge bounds. Both account for latitude and real physical sample spacing.
- Parallel rows combine native fine and broader normals, northwest directional
  light, terrain horizon shadows and multi-scale valley skylight occlusion.
  Shadow strength follows actual heights rather than procedural texture/noise.
- `elevation_color` interpolates land/depth pigments, with subdued clay/ochre
  mountains and pale summits. Invalid neighbours use the centre height.

## Contracts
Metres outside -11,000..9,000 or nonfinite samples are transparent. Dimensions
must match the sample count and be at least 3×3. Regional edges clamp; only a full
360-degree grid wraps horizontally. Physical spacing comes from the actual image
bounds, so a native crop never reuses world-overview lighting dimensions.
Exaggeration affects illumination only, not geography, data or marker placement.
