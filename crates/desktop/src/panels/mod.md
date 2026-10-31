# mod.rs

## Purpose
Exports the panel renderers used by the desktop shell. This module is the composition boundary between `app.rs` and the individual view files.

## Components

### Re-exports
- **Does**: Exposes `render_settings`, `render_factal_brief`, `render_event_list`, `render_layer_drawer`, and `render_world_map`
- **Interacts with**: `app.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | Renderer functions stay re-exported from here | Removing or renaming exports |

`layer_file_import` owns asynchronous user-uploaded map-layer parsing.

Exports `render_settings` as the only global settings entry point. The retired
header and separate Terrain Library window exports are removed. `terrain_library`
exposes background ticking to the app; `map_rail` owns compact map controls.

`event_chyron` owns the five-row Follow Events crawl shared by the map overlay
and the Brief's reserved bottom space.
