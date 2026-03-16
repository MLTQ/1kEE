# app.rs

## Purpose
Coordinates the desktop app at the highest level. It owns the root `AppModel`, installs visuals, and composes the major UI panels.

## Components

### `DashboardApp`
- **Does**: Stores shared UI state for the running app
- **Interacts with**: `AppModel` in `model.rs`, panel renderers in `panels/`, shutdown cleanup in `panels/world_map/srtm_focus_cache.rs`

### `DashboardApp::new`
- **Does**: Installs theme configuration and seeds the demo model
- **Interacts with**: `install` in `theme.rs`, `AppModel::seed_demo` in `model.rs`

### `Drop for DashboardApp`
- **Does**: Cancels active GDAL terrain-cache subprocesses when the native app is tearing down
- **Interacts with**: `terminate_active_gdal_jobs` in `panels/world_map/srtm_focus_cache.rs`
- **Rationale**: Prevents orphaned `gdalwarp` / `gdal_contour` workers from surviving app shutdown and continuing unsupervised

### `eframe::App::update`
- **Does**: Lays out the shell around header, terrain-library window, sidebars, status log, and map canvas
- **Interacts with**: `render_*` functions in `panels/mod.rs`
- **Rationale**: Keeps app composition separate from the details of each view

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `main.rs` | `DashboardApp::new` returns a ready-to-render app | Constructor signature |
| `panels/*` | Shared state lives in `AppModel` and remains mutable here | Moving state ownership elsewhere |
| Runtime shutdown | Dropping `DashboardApp` terminates tracked terrain-cache subprocesses | Removing the cleanup hook or making it best-effort elsewhere |
