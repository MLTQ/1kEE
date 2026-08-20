# main.rs

## Purpose

Declares the desktop crate's internal modules and starts the native eframe
application with the map-oriented window configuration.

## Components

### Module declarations
- **Does**: Makes source adapters, model modules, map panels, and application
  lifecycle code available inside the desktop binary.
- **Interacts with**: all crate-local modules.

### `deflock_source`
- **Does**: Declares the cache-first public DeFlock-compatible ALPR adapter.
- **Interacts with**: `model.rs`, `app.rs`, and map overlays.

### `camera_directory_pipeline`

- **Does**: Declares the opt-in Project Eyes On-compatible public-directory
  discovery, verification, and geolocation pipeline.
- **Interacts with**: `camera_registry.rs` and the shared camera model.

### `camera_feed_viewer`

- **Does**: Declares the non-blocking snapshot/MJPEG reader and floating egui
  live-feed window.
- **Interacts with**: `app.rs` and camera selection in `model/mod.rs`.

### `main`
- **Does**: Configures the native window and constructs `DashboardApp`.
- **Interacts with**: `app.rs` and eframe.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `app.rs` | `DashboardApp` is the eframe entrypoint | Changing the startup callback contract |
| Internal modules | Required source modules are declared before use | Removing or privatizing a needed declaration |

## Notes

- This binary intentionally uses WGPU because the world map registers custom
  GPU paint callbacks during app startup.
