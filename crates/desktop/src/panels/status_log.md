# status_log.rs

## Purpose
Renders a compact operator log at the bottom of the screen. It exposes the most recent state transitions so the UI feels like an active operations surface instead of a static mockup.

## Components

### `render_status_log`
- **Does**: Displays recent log lines and the currently selected camera
- **Interacts with**: `AppModel::activity_log` and `selected_camera` in `model.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | Function renders read-only status output in the bottom panel | Changing the entrypoint signature or mutating the model |
