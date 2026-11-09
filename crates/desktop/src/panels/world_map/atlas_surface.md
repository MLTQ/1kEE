# atlas_surface.rs

## Purpose
Define actual elevation-driven Atlas surfaces and their camera transforms.
Native height values survive loading and displace GPU geometry in metres.

## Components
- `Raster` retains measured heights and shaded pigment bytes with immutable IDs.
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

`atlas_surface.wgsl` shares the 224-byte uniform layout, manually interpolates
unfilterable R32Float height samples at their pixel-center coordinates, and
projects the resulting mesh. Local depth is monotonic along the shared affine
camera's ground axis; Globe uses perspective depth. Optional contour, coastline
and bathymetry isolines use the mesh's interpolated elevation with screen-space
antialiasing. The native globe window excludes the overview beneath its extent.
