# atlas_lighting.wgsl

## Purpose
Shade unlit pigments from measured terrain at render time. Geographic movable
studio light or Earth-fixed Sun illuminates the same Globe/Local surface.

## Components
- Fine and broad physical-scale normals combine ridge detail and mountain masses.
- A bounded 16-step height-field ray casts soft terrain shadows toward the light.
- Broad valley enclosure reduces skylight; warm highlights/cool shadows preserve
  pigment color. Color richness and relief contrast are uniform-only controls.

## Contracts
Composed with `atlas_surface.wgsl` through the production shader-source helper.
Pixel derivatives are computed before fragment discard, then passed explicitly.
Spacing accounts for latitude and actual crop dimensions; invalid samples use
valid center height. Full-world longitude wraps; regional shadow rays stop at the
source boundary. This stylized cartographic illumination never alters geometry,
source heights or layer flags. Sunlight includes a subdued night-side fill.
