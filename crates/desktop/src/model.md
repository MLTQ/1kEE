# model.rs

## Purpose
Defines the shared domain and UI state for the 1kEE desktop demo. This file holds normalized event and camera records plus the selection and logging logic that the panels consume.

## Components

### `GeoPoint`
- **Does**: Stores latitude and longitude for events and cameras
- **Interacts with**: `haversine_km`, `world_map.rs`

### `GlobeViewState`
- **Does**: Stores persistent globe orbit, local-terrain viewport center, local camera angles, local contour layer spread, zoom, and auto-spin state
- **Interacts with**: `world_map/camera.rs`, `world_map/globe_scene.rs`, `world_map/local_terrain_scene.rs`, `AppModel::select_event`
- **Rationale**: Keeps both navigation modes stable across frames, preserves analyst-tuned local terrain settings, and re-centers the local viewport when the selected event changes

### `EventSeverity`
- **Does**: Encodes event urgency and color semantics
- **Interacts with**: `event_list.rs`, `world_map.rs`

### `EventRecord`
- **Does**: Represents one curated event shown in the analyst UI, whether seeded locally or synced from Factal
- **Interacts with**: `AppModel`, `event_list.rs`, `factal_stream.rs`

### `FactalBrief`
- **Does**: Stores the richer Factal-only payload preserved for inspection, including severity value, vertical/subvertical tags, point WKT, topic names, and the pretty-printed raw JSON item
- **Interacts with**: `factal_stream.rs`, `factal_brief.rs`, `world_map.rs`

### `CameraConnectionState`
- **Does**: Tracks the current feed reachability/attempt state for a camera
- **Interacts with**: `camera_list.rs`, `status_log.rs`

### `CameraFeed` / `NearbyCamera`
- **Does**: Store normalized registry data and precomputed nearby-camera views for the selected event
- **Interacts with**: `AppModel::nearby_cameras`, `AppModel::replace_camera_registry`, `world_map.rs`, `camera_list.rs`, `camera_registry.rs`

### `AppModel`
- **Does**: Owns all shared demo state and handles live Factal event replacement, live camera-registry replacement, settings-window UI state, manual city focus, terrain-library UI state, road-layer visibility state, OSM source/runtime status, and simulated feed actions
- **Interacts with**: `app.rs`, every renderer in `panels/`, `TerrainInventory` in `terrain_assets.rs`, `OsmInventory` in `osm_ingest.rs`, `GlobeViewState`, `city_catalog.rs`, `settings_store.rs`, user-selected asset roots
- **Rationale**: Keeps the current scaffold simple while preserving a clear seam for background workers like the Factal poller

### `AppModel::has_factal_api_key`
- **Does**: Reports whether a non-empty Factal token is currently configured
- **Interacts with**: `app.rs`, `factal_settings.rs`

### `AppModel::replace_factal_events`
- **Does**: Swaps in a fresh live event list while retaining the current selection when possible
- **Interacts with**: `factal_stream.rs`, `event_list.rs`, `world_map.rs`
- **Rationale**: Lets the live poller refresh the operational picture without resetting the whole UI on every minute tick

### `AppModel::replace_camera_registry`
- **Does**: Swaps in a fresh live camera registry while retaining the current camera selection when possible
- **Interacts with**: `camera_registry.rs`, `camera_list.rs`, `world_map.rs`
- **Rationale**: Lets live camera-source sync replace the mock/demo catalog without resetting the operator’s current context

### `haversine_km`
- **Does**: Computes distance between two coordinates
- **Interacts with**: `AppModel::nearby_cameras`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `AppModel::seed_demo` returns a ready state | Constructor removal or signature change |
| `camera_list.rs` | `nearby_cameras` returns distance-sorted items | Changing sort order or field names |
| `world_map.rs` | `selected_event`, `nearby_cameras`, and `cameras` remain available | Renaming state accessors or moving map data out |
| `header.rs` | `terrain_inventory` is available for top-level dataset status | Removing or relocating terrain status state |
| `header.rs` | `osm_inventory` is available for top-level OSM source/runtime status | Removing or relocating OSM status state |
| `world_map/camera.rs` | `globe_view` is available for persistent camera interaction | Removing or relocating globe state without replacing the contract |
| `header.rs` | `selected_root` can be updated from the UI to re-resolve terrain assets | Removing or relocating root-selection state without replacing the contract |
| `terrain_library.rs` | City focus, search text, and selected city ids live here and can be mutated from UI actions | Removing or relocating terrain-library state without replacing the contract |
| `factal_settings.rs` | Factal key text, path override text, and window-open state live here and can be mutated from UI actions | Removing or relocating settings state without replacing the contract |
| `factal_stream.rs` | `replace_factal_events` swaps in fresh event lists without destroying other app state | Removing the method or changing its selection-retention semantics |

## Notes
- This is still a single-threaded demo model.
- Real event and camera ingest should eventually populate this state through dedicated adapter layers instead of `seed_demo`.
- Terrain inventory is deliberately lightweight and should eventually point at preprocessed runtime assets, not raw source rasters.
- The seeded default focus now starts in San Francisco so the local terrain renderer can be tuned against steeper urban relief.
- Manual city focus now coexists with the event demo: selecting a city re-centers terrain without destroying the seeded event list, and selecting an event clears the manual city focus again.
- Manual city focus labels now use region-qualified city names when the GeoNames catalog can resolve an admin1/state entry, so repeated place names are less ambiguous in the header, logs, and terrain library.
- Factal API key persistence is intentionally lightweight for now: the key is loaded into the model at startup and the live poller swaps in fresh events once authenticated.
- Factal-backed events now preserve an optional raw-detail payload so the operator can inspect the original API item from a brief window without bloating the normalized event list UI.
- Live camera-source keys now also persist in the model/settings path, and the camera registry status is explicit about `demo` vs `configured` vs `live` instead of treating mock cameras as a loaded source.
- Nearby-camera queries are memoized per selected event and camera-registry generation, and the render-facing list is capped to the nearest 200 cameras so one dense event does not stall every frame.
- The globe now starts in manual mode instead of auto-spin so the app does not enter a continuous repaint loop before the analyst touches anything.
- The model now initializes and tracks a separate OSM runtime store so the planet-scale roads/buildings pipeline can evolve independently from terrain caching.
- Coastline and major/minor road layer toggles now live in the model because both the map UI and the renderers need the same persistent visibility state.
- Local terrain now defaults `local_layer_spread` to `1.0`, which is the neutral baseline after the projection fix; operators can still push exaggeration far beyond that from the footer control, now up to `100.0`.
- Path settings now persist through the shared settings store and default to the executable directory rather than the process working directory.
- Globe pitch now clamps near the poles instead of around `±63°`, so focus and drag behavior can reach the high Arctic and Antarctic without flipping through the poles.
