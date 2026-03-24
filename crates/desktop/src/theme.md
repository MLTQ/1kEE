# theme.rs

## Purpose
Centralizes the visual language for the 1kEE desktop demo. It installs the active `egui` theme, exposes the selectable map palettes, and provides semantic color helpers that keep chrome, overlays, and map layers aligned.

## Components

### `MapTheme`
- **Does**: Enumerates the available palette presets, including the optional sodium-lamp amber theme
- **Interacts with**: `factal_settings.rs`, `app.rs`, `model.rs`

### `install`
- **Does**: Applies spacing, backgrounds, and interactive widget colors to the `egui` context
- **Interacts with**: `DashboardApp::new` in `app.rs`

### Color helper functions
- **Does**: Provide consistent panel, grid, road, camera, muted-text, and globe/HUD accent colors
- **Interacts with**: panel renderers in `panels/`, especially `world_map.rs` and `road_layer.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `install` mutates the `egui::Context` safely during startup | Removing the function or changing its side effects materially |
| `panels/*` | Helper colors remain stable semantic tokens, including the globe wireframe and road-overlay palette | Renaming or removing helper functions |

## Notes
- `Topo` remains the startup/default theme; `Sodium` is an additional selectable option in Settings.
- The sodium palette intentionally uses true-black backgrounds with warm amber highlights so the app can read like a low-pressure street-lamp display without changing existing themes.
