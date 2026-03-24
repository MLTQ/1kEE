# zoom.rs

## Purpose
Maps terrain zoom levels onto contour-cache LOD settings. This file centralizes the half-extent, interval, simplification, and budget policy used by both builders and overlays.

## Components

### `feature_budget_for_zoom` / `half_extent_for_zoom` / `zoom_bucket_for_zoom` / `contour_interval_for_zoom`
- **Does**: Expose zoom-derived cache settings to callers that only need one semantic value
- **Interacts with**: `contour_asset.rs`, `local_terrain_scene.rs`

### `bucket_radius_for_target_radius_miles`
- **Does**: Converts a real-world radius request into the discrete bucket radius needed by the cache
- **Interacts with**: regional contour prefetch callers

### `spec_for_zoom`
- **Does**: Returns the full LOD specification for one zoom value
- **Interacts with**: `mod.rs`, `builders.rs`, `gdal.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| Cache builders | Zoom buckets remain monotonically more detailed as zoom increases | Reordering the ladder or changing bucket semantics abruptly |
| UI overlays | Contour interval and half-span remain aligned with the actual generated tiles | Diverging overlay text from builder settings |
