# theme.rs

## Purpose
Centralizes the visual language for the 1kEE desktop demo. It installs the active `egui` theme, exposes the selectable map palettes, and provides semantic color helpers that keep chrome, overlays, and map layers aligned.

## Components

### `MapTheme`
- **Does**: Enumerates the available palette presets, including the optional sodium-lamp amber theme
- **Interacts with**: `settings_appearance.rs`, `app.rs`, `model.rs`

### `install`
- **Does**: Applies spacing, backgrounds, and interactive widget colors to the `egui` context
- **Interacts with**: `DashboardApp::new` in `app.rs`

### Color helper functions
- **Does**: Provide consistent panel, grid, road, camera, muted-text, and globe/HUD accent colors. Camera pips and selection rings use a stable green identity across themes.
- **Interacts with**: panel renderers in `panels/`, especially `world_map.rs` and `road_layer.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `install` mutates the `egui::Context` safely during startup | Removing the function or changing its side effects materially |
| `panels/*` | Helper colors remain stable semantic tokens, including the globe wireframe and road-overlay palette | Renaming or removing helper functions |

## Notes
- `Topo` remains the startup/default theme; `Sodium` is an additional selectable option in Settings.
- The sodium palette intentionally uses true-black backgrounds with warm amber highlights so the app can read like a low-pressure street-lamp display without changing existing themes.
- Roads use subdued, opaque roadbed tones rather than bright accents: muted clay
  in Topo, dusty crimson in Akira/Mars Dark, and matching low-saturation tones in
  other palettes. Minor roads sit nearer the background; live markers remain
  brighter. Opaque colors avoid accumulating brightness where road segments join.

Each theme defines ten level colors in `theme/admin.rs`. The checkbox labels,
Globe and Local outlines, and region shading read the selected theme's palette.
Base colors are opaque; the separate boundary/shading opacity controls provide
transparency. Country borders retain the brightest color and widest stroke.

Native Earth terrain applies these colors at draw time: Globe recolors retained
class-coverage images, and Local updates major/minor uniforms. Palette changes
do not invalidate source tiles or wait for geometry rebuilds/uploads.

### Atlas
`ATLAS` uses warm paper controls and dark ink text in daylight. When the Sun is
below the horizon at the center of the Earth Globe or Local view, the controls
use the navy Topo palette. The selected map palette still supplies Atlas mineral
contours, subdued roads and blue water. `is_atlas` also identifies the measured
relief base in the globe pass. Light overlay fills use unmultiplied input so
translucent parchment remains valid premultiplied color. The default stays Topo.

The UI checks the Atlas clock, including the Follow Events recap playhead.
Sunlight mode applies its preview offset to both terrain and controls. Studio
lighting uses the unshifted clock for this check.
