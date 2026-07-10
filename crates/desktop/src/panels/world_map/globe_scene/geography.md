# geography.rs

## Purpose

Composes globe geography: contour-derived terrain, coastline, and bathymetry
layers, plus geographic overlay helpers and planetary labels.

## Components

### `paint_contour_layer`

- **Does**: Converts a cached contour batch into the GPU callback for one
  frame, forwarding fade, elevation offset, and the shared stroke scale.
- **Interacts with**: `world_map/contour_pass.rs`.

### Terrain and planetary contour functions

- **Does**: Load/colour Earth, lunar, and Martian vector contours and submit
  them through the common GPU path.
- **Interacts with**: `globe_scene/mod.rs`, `contour_asset.rs`, and theme
  palette helpers.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Globe scene | All contour-derived globe line layers accept the same validated scale | Scaling only one body or contour type |
| Contour pass | A `1×` scale preserves legacy width and avoids cache rebuilds | Baking scale into palette or geometry cache keys |

## Notes

- GeoJSON, labels, and non-contour linework intentionally do not consume this
  setting.
- Lunar and Martian feature labels remain available when the Contours toggle is
  off; the toggle suppresses only the terrain linework.
