# mod.rs

## Purpose
Owns the modularized globe renderer for the world map panel. It coordinates globe layout, terrain and coastline passes, marker rendering, and hit-test data while delegating focused responsibilities to sibling files.

## Components

### `GlobeScene`
- **Does**: Returns the visible marker positions and beam metadata needed by `world_map.rs`
- **Interacts with**: `world_map.rs`

### `paint`
- **Does**: Draws the globe backdrop, terrain/coastline context, HUD chrome, markers, and ArcGIS overlays for the current frame
- **Interacts with**: `AppModel` in `model.rs`, `geography.rs`, `markers.rs`, `projection.rs`, theme helpers in `theme.rs`
- **Rationale**: Keeps the world-map entrypoint small while preserving a single public render contract

### Layout and interaction helpers
- **Does**: Convert view state into per-frame projection geometry and legend/HUD overlays
- **Interacts with**: `camera.rs`, `graticule.rs`, `projection.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `world_map.rs` | `paint` returns the same marker contract as local terrain mode | Changing the return structure or hit-test semantics |
| `projection.rs` / `markers.rs` / `geography.rs` | `GlobeLayout` and `ProjectedPoint` remain the shared interchange types | Renaming or removing those structs |

## Notes
- This directory-backed module is now the active globe implementation; the older sibling `globe_scene.rs` remains in the tree but is no longer the path selected by `world_map.rs`.
