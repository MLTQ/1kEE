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

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| World map | Toggle values correspond directly to renderer visibility checks | Relabeling a control while changing its target field |
| Cache helpers | Disabling road/water layers triggers the appropriate invalidation behavior | Removing cache invalidation on affected toggles |
| Optional sources | A layer can remain off while its cache/data becomes available | Making a source fetch conditional on visual enablement alone |

## Notes

- Public-source controls should describe their provenance/status without
  triggering network work inside this rendering function.
