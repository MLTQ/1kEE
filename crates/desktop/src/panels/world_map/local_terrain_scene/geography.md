# geography.rs

## Purpose

Draws Earth coastline and bathymetry contours in the local oblique-terrain
projection, using the same cached vector geometry as the globe scene.

## Components

### `draw_bathymetry_local` / `draw_coastlines_local`

- **Does**: Cull global paths to the local viewport, project them to screen
  space, and paint their styled strokes.
- **Interacts with**: `contour_asset.rs`, local `projection.rs`, and the local
  scene entrypoint.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Layer drawer | The physical primary width is converted once to the shared local scale, changing coastline and bathymetry alongside local contours while retaining a 1 px floor | Omitting that derived scale or floor from either helper |
| Local terrain scene | Roads, boundaries, and other non-contour strokes retain their own widths | Applying the contour scale to unrelated overlays |

## Notes

- The local scene converts the operator's physical primary width back to the
  legacy scale before these helpers run, retaining the original 1.2/0.7
  bathymetry and 1.0 coastline proportions above the one-pixel floor.
