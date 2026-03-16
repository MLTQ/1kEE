# mod.rs

## Purpose
Exports the panel renderers used by the desktop shell. This module is the composition boundary between `app.rs` and the individual view files.

## Components

### Re-exports
- **Does**: Exposes `render_header`, `render_terrain_library`, `render_status_log`, `render_event_list`, `render_camera_list`, and `render_world_map`
- **Interacts with**: `app.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | Renderer functions stay re-exported from here | Removing or renaming exports |
