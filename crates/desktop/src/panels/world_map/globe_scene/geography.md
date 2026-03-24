# geography.rs

## Purpose
Draws geographic context layers on the globe. This file handles coastlines, bathymetry, low-zoom topo contours, and SRTM-on-globe overlays.

## Components

### `draw_global_coastlines`
- **Does**: Paints the cached coarse coastline overlay on the globe
- **Interacts with**: `contour_asset.rs`, `projection.rs`

### `draw_global_bathymetry`
- **Does**: Renders GEBCO depth fill plus isobath contours over ocean regions
- **Interacts with**: `gebco_depth_fill.rs`, `projection.rs`

### `draw_global_topo`
- **Does**: Paints the coarse low-zoom global contour context with zoom-based fadeout
- **Interacts with**: `contour_asset.rs`, theme tokens in `theme.rs`

### `draw_srtm_on_globe`
- **Does**: Projects streamed SRTM contour tiles onto the globe surface at mid zoom levels
- **Interacts with**: `contour_asset.rs`, `projection.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `globe_scene/mod.rs` | These helpers are pure paint steps over an existing globe layout | Changing them to own frame lifecycle or hit-testing |
| `projection.rs` | Geographic paths are projected with globe-aware front/back handling | Bypassing projection helpers or changing path semantics |
