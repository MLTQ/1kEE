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
| Layer drawer | The shared contour scale changes coastline and bathymetry width in local mode as it does on the globe | Omitting the scale from either helper |
| Local terrain scene | Roads, boundaries, and other non-contour strokes retain their own widths | Applying the contour scale to unrelated overlays |

## Notes

- At `1×`, the original 1.2/0.7 bathymetry and 1.0 coastline widths are
  unchanged.
