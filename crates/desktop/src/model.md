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
- **Does**: Represents one curated event shown in the analyst UI
- **Interacts with**: `AppModel`, `event_list.rs`

### `CameraConnectionState`
- **Does**: Tracks the current feed reachability/attempt state for a camera
- **Interacts with**: `camera_list.rs`, `status_log.rs`

### `CameraFeed` / `NearbyCamera`
- **Does**: Store registry data and precomputed nearby-camera views for the selected event
- **Interacts with**: `AppModel::nearby_cameras`, `world_map.rs`, `camera_list.rs`

### `AppModel`
- **Does**: Owns all shared demo state and handles selection and simulated feed actions
- **Interacts with**: `app.rs`, every renderer in `panels/`, `TerrainInventory` in `terrain_assets.rs`, `GlobeViewState`, user-selected asset roots
- **Rationale**: Keeps the current scaffold simple while preserving a clear seam for future background workers

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
| `world_map/camera.rs` | `globe_view` is available for persistent camera interaction | Removing or relocating globe state without replacing the contract |
| `header.rs` | `selected_root` can be updated from the UI to re-resolve terrain assets | Removing or relocating root-selection state without replacing the contract |

## Notes
- This is still a single-threaded demo model.
- Real event and camera ingest should eventually populate this state through dedicated adapter layers instead of `seed_demo`.
- Terrain inventory is deliberately lightweight and should eventually point at preprocessed runtime assets, not raw source rasters.
- The seeded default focus now starts in San Francisco so the local terrain renderer can be tuned against steeper urban relief.
