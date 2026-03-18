# terrain_library.rs

## Purpose
Renders the searchable city precompute and terrain-focus window. This file exists to give operators one place to search cities, queue cache warming, and inspect ongoing background terrain downloads.

## Components

### `render_terrain_library`
- **Does**: Advances background terrain and OSM jobs, draws the library window, handles city search/selection/focus actions, renders region-qualified city labels, and lists contour/OSM job progress
- **Interacts with**: `AppModel` in `model.rs`, `city_catalog.rs`, `terrain_precompute.rs`, `osm_ingest.rs`

### `draw_job_row`
- **Does**: Renders one city precompute job with a progress bar and state label
- **Interacts with**: `PrecomputeJobSnapshot` in `terrain_precompute.rs`

### `draw_osm_job_row`
- **Does**: Renders one OSM ingest job with its current state and worker note
- **Interacts with**: `OsmJobSnapshot` in `osm_ingest.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `render_terrain_library` can be called every frame and will keep background jobs moving | Making the renderer blocking or moving job advancement elsewhere |
| `header.rs` | Opening the terrain library toggles `AppModel::terrain_library_open` | Renaming the open-state field without updating callers |

## Notes
- The visual model is intentionally close to a lightweight downloads manager rather than a settings form.
- The first pass now queries a local GeoNames-derived SQLite catalog and uses an additive precompute queue; the lazy terrain stream still handles anything not prewarmed.
- Search rows now prefer region-qualified labels when the catalog can resolve them, which is especially important for repeated U.S. city/place names.
- The same window now also surfaces explicit OSM bootstrap actions because planet-scale road/building extraction is heavyweight enough that it should be operator-controlled and visible.
- `Queue Focus Roads` is the preferred first action because it targets the current terrain focus and is scheduled ahead of the slower global backfill.
- The OSM capability note is still important: `LocationsOnWays` controls whether the pure-Rust full-planet roads bootstrap can run, but focused road imports can fall back to a bounded `ogr2ogr` extraction.
