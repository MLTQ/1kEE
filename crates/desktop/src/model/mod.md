# mod.rs

## Purpose

Defines the central UI-thread `AppModel`, re-exports its focused domain model
modules, and coordinates selection, settings, live-source replacement, and map
state. It is the shared state contract used by desktop panels and non-blocking
source pollers.

## Components

### `AppModel::seed_demo`
- **Does**: Initializes settings, asset inventories, demo records, map state,
  and source status for a usable first frame.
- **Interacts with**: settings, terrain/OSM inventories, and the model domain
  modules.

### Selection and replacement methods
- **Does**: Maintain valid event/camera selection while focus, event feeds, and
  camera registries change.
- **Interacts with**: camera list, event list, source pollers, and world map
  panels.

### `nearby_cameras` / `nearby_camera_snapshot`
- **Does**: Returns cameras within a radius of the selected event, sorted by
  haversine distance; map call sites share an immutable cached snapshot.
- **Interacts with**: camera sidebar and globe/local terrain marker rendering.

### DeFlock ALPR snapshot fields
- **Does**: Hold an immutable public OpenStreetMap ALPR snapshot, visibility
  toggle, and source status without embedding fetch logic in map renderers.
- **Interacts with**: `deflock_source.rs` and the world-map ALPR overlay.

### `replace_deflock_alpr_locations`

- **Does**: Replaces the public ALPR snapshot and advances a monotonic revision
  used by map meshes to prevent allocator-address cache collisions.
- **Interacts with**: `deflock_source.rs` and globe/local mesh caches.

### `haversine_km`
- **Does**: Calculates the map's canonical great-circle camera/event distance.
- **Interacts with**: `nearby_cameras` and camera list labels.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Map scenes | Selection, display toggles, and live data form a consistent immutable snapshot per paint pass | Renaming fields or changing selection semantics |
| Source pollers | `replace_*` methods retain valid selection and report source state | Removing replacement methods or changing their ownership contracts |
| Camera sidebar | Nearby records are sorted ascending by exact `haversine_km` results | Changing distance calculation, inclusion boundary, or sort order |
| Gruve bridge | Public model fields can be snapshotted and commands can update supported selection state | Removing required state or thread-affecting changes |

## Notes

- `AppModel` is UI-owned; background pollers return outcomes that the UI thread
  applies through the replacement methods.
- Camera/event data may update independently, so derived nearby-camera data is
  keyed by selected-event coordinates, radius, and an internal camera-registry
  revision before it can be reused.
