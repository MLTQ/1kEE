# mod.rs

## Purpose

Defines the central UI-thread `AppModel`, re-exports its focused domain model
modules, and coordinates selection, settings, live-source replacement, and map
state. It is the shared state contract used by desktop panels and non-blocking
source pollers.

## Components

### `AppModel::new`
- **Does**: Initializes settings, asset inventories, empty event/camera
  collections, neutral map state, and honest source status.
- **Interacts with**: settings, terrain/OSM inventories, and the model domain
  modules.
- **Rationale**: Runtime domain records must come from live or imported sources;
  the application initializer never fabricates operational data.

### Contour stroke width

- **Does**: Holds an optional physical primary contour width and derives the
  legacy relative scale local painters need. Older multiplier-only settings
  resolve at the active display density, preserving visible output while
  raising sub-pixel strokes to the new one-pixel floor.
- **Interacts with**: `settings_store.rs`, the layer drawer, globe contour
  callbacks, and local-terrain contour painters.

### Selection and replacement methods
- **Does**: Maintain valid event/camera selection while focus, event feeds, and
  camera registries change. The first authentic event becomes the initial map
  focus unless the operator has already selected a city.
- **Interacts with**: camera list, event list, source pollers, and world map
  panels.

### `AppModel::open_camera_feed`

- **Does**: Selects a valid camera, records the connection attempt, and opens
  the floating live-feed window.
- **Interacts with**: map pip clicks, the camera list, and
  `camera_feed_viewer.rs`.

### Project Eyes On settings and camera visibility

- **Does**: Holds the explicit directory-pipeline opt-in, optional country
  scope, configurable requests-per-minute pace, and camera-dot visibility.
- **Interacts with**: `settings_store.rs`, `camera_registry.rs`,
  `factal_settings.rs`, and both world-map scenes.

### Camera registry progress

- **Does**: Holds the active scan flag, normalized completion fraction, and
  current discovery/checking counters for the top-bar progress indicator.
- **Interacts with**: `camera_registry.rs` and `panels/header.rs`.

### `nearby_cameras` / `nearby_camera_snapshot`
- **Does**: Returns cameras within a radius of the selected event, sorted by
  haversine distance; map call sites share an immutable cached snapshot.
- **Interacts with**: camera sidebar and globe/local terrain marker rendering.

### DeFlock ALPR snapshot fields
- **Does**: Hold an immutable public OpenStreetMap ALPR snapshot, visibility
  toggle, and source status without embedding fetch logic in map renderers.
- **Interacts with**: `deflock_source.rs` and the world-map ALPR overlay.

### Flock inventory
- `flock` owns the local TSV reader state, compact snapshot, independent visibility
  and service-status filter. It starts empty and loads only authentic file data.
- `public_camera_positions` chains enabled DeFlock and Flock positions for batched
  rendering without mixing their identities, metadata or source attribution.

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
| `app.rs` | `AppModel::new` starts with no events, cameras, or selections | Seeding runtime domain records or restoring placeholder URLs |
| Source pollers | `replace_*` methods retain valid selection and report source state | Removing replacement methods or changing their ownership contracts |
| Camera sidebar | Nearby records are sorted ascending by exact `haversine_km` results | Changing distance calculation, inclusion boundary, or sort order |
| Camera registry | `has_enabled_camera_sources` includes both keyed adapters and the explicit Project Eyes On opt-in | Ignoring the keyless directory setting |
| Header | Camera scan progress is UI-thread state updated only by drained worker messages | Mutating egui state from registry workers |
| Camera feed viewer | `selected_camera_id` and `camera_feed_window_open` identify the one requested feed | Changing window-state semantics without updating the viewer |
| Map renderers | `contour_stroke_width_px()` is at least one physical pixel; local scale conversion preserves existing major/minor relationships | Bypassing the setter or mixing physical width with logical-point stroke math |
| Gruve bridge | Public model fields can be snapshotted and commands can update supported selection state | Removing required state or thread-affecting changes |

## Notes

- `AppModel` is UI-owned; background pollers return outcomes that the UI thread
  applies through the replacement methods.
- Startup contains no synthetic event or camera records. Inactive sources leave
  their lists empty until authentic data arrives.
- Camera/event data may update independently, so derived nearby-camera data is
  keyed by selected-event coordinates, radius, and an internal camera-registry
  revision before it can be reused.
- New contour widths are normalized to a visible physical-pixel range. The
  legacy multiplier remains private only as a settings migration path, so a
  missing newer field does not change existing map output.

Public pipelines use `show_pipeline`, `pipeline_filters` (source/product/status), and `pipeline_osm` for the independent local OSM layer. Filters start with historical/planned records hidden; unknown status remains visible.

- Offshore platform visibility defaults off; `platform_filters` keeps public-source and historic/planned/support installation options independent of pipelines.

### Startup layer visibility

Events, Webcams, Contours, Bathymetry and Coastline start enabled. Flock, graticule, reticle, the local targeting beam and stellar/planet overlays start hidden alongside the other optional layers. Layer controls can enable them during the session; source/product filters remain independent of parent-layer visibility.

### Event-follow selection

`event_follow` observes only completed live Factal snapshots, independently of USGS
refreshes and historical replay. `tick_event_follow` selects the new target and
opens its brief without snapping the camera; replacement cannot refocus an active
flight. A followed record remains available for its brief after paging out.
Manual event/city selection, replay, leaving Earth or leaving cinematic mode stops
following. Webcams default on; the independent Flock inventory remains off.

`map_events` includes the followed target after it has paged out, so its marker and brief stay tied to the continuing orbit without repopulating the live feed list.

FOLLOW EVENTS now tours the six highest-severity records from each successful
Factal payload. Every timed stop uses the same `tick_event_follow` selection and
brief-opening path; source replacement still cannot snap the active camera.

### Marker size
`marker_scale()` and `set_marker_scale()` own the normalized 25–200% webcam/event
size. Startup, settings reload and save retain it independently of layer toggles.
