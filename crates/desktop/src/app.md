# app.rs

## Purpose

Owns the desktop application's egui lifecycle, initializes shared rendering and
data services, and advances non-blocking source pollers each frame. It is the
UI-thread coordinator for `AppModel`.

## Components

### `DashboardApp::new`
- **Does**: Installs the theme, starts GPU callbacks and local services, then
  constructs the initial model and companion bridge.
- **Interacts with**: `model.rs`, GPU world-map passes, event storage, and
  `GruveBridge`.

### `DashboardApp::update`
- **Does**: Drains companion commands, ticks source adapters, updates animation
  state, renders the desktop panels, and advances the non-blocking live-camera
  viewer.
- **Interacts with**: source modules, `AppModel`, `panels`, and
  `camera_feed_viewer.rs`.

### Camera feed viewer ownership

- **Does**: Keeps media-worker and GPU-texture state outside the serializable
  domain model while rendering its egui window after the central map.
- **Interacts with**: `CameraFeedViewer` and camera window state in `AppModel`.

### DeFlock source lifecycle
- **Does**: Ticks the cache-first public ALPR adapter and signals its workers on
  shutdown alongside other source adapters.
- **Interacts with**: `deflock_source.rs`.

### `Drop for DashboardApp`
- **Does**: Signals background source workers and active terrain jobs to stop.
- **Interacts with**: source shutdown hooks and world-map terrain cache.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Source adapters | `tick` is called on the UI thread every frame and must be non-blocking | Adding a blocking source call to `update` |
| Source workers | Shutdown hooks run before application teardown | Removing shutdown calls |
| Panels | Model mutations from completed source work are visible before rendering | Moving poller ticks after panel rendering |
| Camera viewer | Viewer render runs every frame after map clicks can request a feed | Moving it before map interaction |

## Notes

- Network or disk-heavy source work belongs in source-module workers; this file
  only schedules and applies ready results.
