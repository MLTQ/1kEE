# atlas_image.rs

## Purpose
Convert a global, north-up GEBCO height grid into a vintage shaded-relief image.
Blue bathymetry, parchment lowlands, sage/ochre foothills, clay mountains and pale
summits are elevation tints, not geological or land-cover classifications.

## Components
- `HeightGrid::image` applies northwest lighting to measured central-difference
  slopes. Horizontal spacing accounts for latitude, longitudes wrap and poles
  clamp. Invalid/missing neighbours use the centre height to avoid false cliffs.
- `elevation_color` interpolates continuous land and ocean ramps. Sea level is
  intentionally a distinct land/water transition.

## Contracts
Grid samples run west to east, north to south, with global pixel-edge coverage.
Metres outside -11,000..9,000 or nonfinite samples are transparent. Dimensions
must be at least 3×3 and match the sample count. Exaggeration changes lighting
only; it never changes geometry, marker placement or recorded heights.
