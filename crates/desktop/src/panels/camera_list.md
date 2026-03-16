# camera_list.rs

## Purpose
Presents the current set of nearby camera records for the focused event and lets the analyst select a record or simulate a feed connection attempt.

## Components

### `render_camera_list`
- **Does**: Shows sorted nearby cameras with provider, type, distance, health, and action buttons
- **Interacts with**: `AppModel::nearby_cameras`, `select_camera`, and `attempt_connect` in `model.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | Function renders the right sidebar and can mutate camera selection/state | Changing the entrypoint signature or removing action controls |
