# terrain_library.rs

## Purpose
Renders the searchable city precompute and terrain-focus window. This file exists to give operators one place to search cities, queue cache warming, and inspect ongoing background terrain downloads.

## Components

### `render_terrain_library`
- **Does**: Advances background precompute jobs, draws the library window, handles city search/selection/focus actions, and lists job progress
- **Interacts with**: `AppModel` in `model.rs`, `city_catalog.rs`, `terrain_precompute.rs`

### `draw_job_row`
- **Does**: Renders one city precompute job with a progress bar and state label
- **Interacts with**: `PrecomputeJobSnapshot` in `terrain_precompute.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `render_terrain_library` can be called every frame and will keep background jobs moving | Making the renderer blocking or moving job advancement elsewhere |
| `header.rs` | Opening the terrain library toggles `AppModel::terrain_library_open` | Renaming the open-state field without updating callers |

## Notes
- The visual model is intentionally close to a lightweight downloads manager rather than a settings form.
- The first pass now queries a local GeoNames-derived SQLite catalog and uses an additive precompute queue; the lazy terrain stream still handles anything not prewarmed.
