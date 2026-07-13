# layer_drawer.rs

## Purpose

Renders the operator-facing map-layer drawer and mutates only the corresponding
`AppModel` display toggles. It groups terrain, transport, infrastructure, and
optional external overlays without owning their data pipelines.

## Components

### `render_layer_drawer`
- **Does**: Draws layer controls, source-specific enablement, and imported
  layer/ArcGIS source management when the drawer is open.
- **Interacts with**: `AppModel`, world-map cache invalidation helpers, and
  ArcGIS source state.

### `section_label`
- **Does**: Provides consistent compact section headers.
- **Interacts with**: the layer drawer layout.

### DeFlock / OSM ALPR control

- **Does**: Exposes the optional public ALPR overlay, its cache/refresh state,
  and explicit DeFlock/OpenStreetMap attribution links.
- **Interacts with**: `AppModel::show_deflock_alprs` and `deflock_source.rs`.

### Contour thickness control

- **Does**: Exposes a linear physical-pixel contour-width slider (`1..=16 px`,
  quarter-pixel steps) beneath the contour toggle and persists a completed
  adjustment locally.
- **Interacts with**: `AppModel::set_contour_stroke_width_px` and
  `AppModel::save_settings`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| World map | Toggle values correspond directly to renderer visibility checks | Relabeling a control while changing its target field |
| Cache helpers | Disabling road/water layers triggers the appropriate invalidation behavior | Removing cache invalidation on affected toggles |
| Optional sources | A layer can remain off while its cache/data becomes available | Making a source fetch conditional on visual enablement alone |
| Contour renderers | No contour-derived local or globe stroke is below 1 px; local major/minor, coastline, and bathymetry strokes retain their hierarchy above that floor | Sending an unchecked physical width or allowing sub-pixel strokes |

## Notes

- Public-source controls should describe their provenance/status without
  triggering network work inside this rendering function.
- Slider drags update the live view each frame but defer the settings-file
  write until release, avoiding repeated disk writes during adjustment.
- Legacy multiplier-only settings display their equivalent current pixel width
  until the operator moves the control. Only old sub-pixel values are raised to
  the new visible minimum.
