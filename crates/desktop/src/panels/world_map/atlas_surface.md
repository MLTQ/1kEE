# atlas_surface.rs

## Purpose
Define actual elevation-driven Atlas surfaces and their camera transforms.
Native height values survive loading and displace GPU geometry in metres.

## Components
- `Raster` retains measured heights and unlit pigment bytes with immutable IDs.
- `Callback::local` copies the exact shared Local projection parameters used
  by terrain contours, including vertical exaggeration and layer spread.
- `Callback::globe` places height at its real Earth-radius displacement.
- Callbacks delegate private depth-tested rendering/staged uploads to the GPU
  module; they never require a depth attachment on egui's UI render pass.

## Contracts
Atlas contour toggles select isolines of the same displaced triangular surface,
so they cannot float above it or show through foreground mountains. Other themes
retain the original vector contour pipeline. Geometry preserves all source posts
up to a 4096×4096 grid; source height/color textures retain native detail up to the
existing GPU/memory limits. World overviews use a smaller globe geometry grid.

`atlas_surface.wgsl` shares the 256-byte uniform layout, manually interpolates
unfilterable R32Float height samples at their pixel-center coordinates, and
projects the resulting mesh. Local depth is monotonic along the shared affine
camera's ground axis; Globe uses perspective depth. Optional contour, coastline
and bathymetry isolines use the mesh's interpolated elevation with screen-space
antialiasing. The native globe window excludes the overview beneath its extent.

`with_style` supplies live geographic light, richness and contrast uniforms; the composed `atlas_lighting.wgsl` shader shades unlit pigments without changing heights, IDs or upload state. The shared uniform layout is now 256 bytes.

`with_forest` accepts coverage only for a matching raster ID/size. Completed R8 coverage blends a forest pigment before the shared lighting/contour pass, so it has exactly the terrain silhouette and occlusion.
