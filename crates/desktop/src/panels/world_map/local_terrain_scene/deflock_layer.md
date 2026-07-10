# deflock_layer.rs

## Purpose

Draws public DeFlock ALPR-camera points after their geographic positions have
already been projected by a globe or local-terrain scene. The module is purely
visual: it owns no import state, cache, network work, or interaction state.

## Components

### `ProjectedAlprMarker`
- **Does**: Carries an ALPR marker's screen position and optional direction
  already transformed through the active map projection for batched drawing.
- **Interacts with**: globe and local terrain scene projection loops.

### `build_mesh` / `draw_mesh`
- **Does**: Builds the visible ALPR icon set as a compact egui mesh with a
  direction cue when available, then submits either a cached or fresh mesh.
- **Interacts with**: `ProjectedAlprMarker` values from both scene modes.

### `ProjectedAlprMarker::new`
- **Does**: Accepts a projection-derived screen-space orientation and drops
  invalid values safely.
- **Interacts with**: `bearing_target` plus each scene's geographic projection.

### `bearing_target`

- **Does**: Produces a nearby geographic point along a public
  clockwise-from-north heading so globe/local projection determines the exact
  on-screen wedge direction.
- **Interacts with**: `direction_degrees` source metadata and scene projection.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Globe scene | Front-facing geographic points and bearing targets can be rendered without source ownership | Adding projection or source loading to this module |
| Local terrain scene | Terrain-projected points retain their surface-aligned screen positions and yaw-aware bearings | Changing marker position interpretation |
| DeFlock importer | Rendering uses only public location/direction metadata supplied by the model | Introducing cache/network ownership or private-data assumptions |

## Notes

- Rendering is behind `AppModel::show_deflock_alprs` at the call sites and is
  empty until the importer provides data.
- A single mesh keeps large public point sets from producing one painter command
  per icon while preserving every supplied visible location. Globe mode caches
  that mesh whenever its immutable snapshot revision and view transform are
  unchanged; local terrain uses the same exact-input cache.
