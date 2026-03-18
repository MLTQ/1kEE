# app.rs

## Purpose
Coordinates the desktop app at the highest level. It owns the root `AppModel`, installs visuals, and composes the major UI panels.

## Components

### `DashboardApp`
- **Does**: Stores shared UI state for the running app
- **Interacts with**: `AppModel` in `model.rs`, panel renderers in `panels/`, `factal_stream.rs`, `camera_registry.rs`, shutdown cleanup in `panels/world_map/srtm_focus_cache.rs`

### `DashboardApp::new`
- **Does**: Installs theme configuration and seeds the demo model
- **Interacts with**: `install` in `theme.rs`, `AppModel::seed_demo` in `model.rs`

### `Drop for DashboardApp`
- **Does**: Cancels active GDAL terrain-cache subprocesses and suppresses new Factal and camera-registry poll scheduling when the native app is tearing down
- **Interacts with**: `shutdown` in `factal_stream.rs`, `shutdown` in `camera_registry.rs`, `terminate_active_gdal_jobs` in `panels/world_map/srtm_focus_cache.rs`
- **Rationale**: Prevents terrain or live-event background work from continuing unsupervised during shutdown

### `eframe::App::update`
- **Does**: Advances the live Factal poll loop, advances the live camera-registry poll loop, and lays out the shell around header, Factal brief window, Factal settings, terrain-library window, sidebars, status log, and map canvas
- **Interacts with**: `tick` in `factal_stream.rs`, `tick` in `camera_registry.rs`, `render_*` functions in `panels/mod.rs`
- **Rationale**: Keeps app composition separate from the details of each view

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `main.rs` | `DashboardApp::new` returns a ready-to-render app | Constructor signature |
| `panels/*` | Shared state lives in `AppModel` and remains mutable here | Moving state ownership elsewhere |
| Runtime shutdown | Dropping `DashboardApp` terminates tracked terrain-cache subprocesses and suppresses new Factal polling | Removing the cleanup hooks or making them best-effort elsewhere |
