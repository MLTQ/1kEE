# job_dispatch.rs

## Purpose
Owns the OSM ingest queue, worker lifecycle, focus-job scheduling, and the small amount of in-memory state needed to keep the runtime store responsive. It is the choke point that decides whether a focused request should become a new road/water import job.

## Components

### `queue_focus_roads_import`
- **Does**: Queues a focused road import for the current view, but now keys dedupe by the covered focus-cell bounds instead of raw focus latitude/longitude
- **Interacts with**: `roads_osmium.rs`, `roads_stream.rs`, `roads_overpass.rs`
- **Rationale**: Nearby pans should reuse the same focused-cell import instead of spawning a fresh road job for every small camera nudge

### `queue_focus_water_import`
- **Does**: Queues the focused water import path for the visible region
- **Interacts with**: `water.rs`

### `tick`
- **Does**: Advances one background OSM worker, updates the active-job note, and publishes data-generation bumps when imports finish
- **Interacts with**: `db.rs`, feature-specific importers, UI overlays

### `initialize_caches`
- **Does**: Hydrates the in-memory note/job caches at startup and now also recovers orphaned `running` jobs left behind by a previous crash
- **Interacts with**: `db.rs`

### Focus-cell helpers
- **Does**: Defines the 1° cell bucketing and persistent extract paths used by focused osmium imports
- **Interacts with**: `roads_osmium.rs`, `water.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `layer_import.rs` | Focused road imports dedupe aggressively and return quickly when an equivalent cell job is already known | Changing the focus-note scheme without updating the callers |
| `roads_osmium.rs` | Focus-cell path helpers stay stable for durable extract reuse | Renaming the extract path format |
| `status_log.rs` / map overlays | `active_job_note` reflects the current background import | Removing note updates from worker transitions |

## Notes
- Focused road jobs are now deduped by the 1° cell envelope they cover plus the radius bucket. That is less precise than the raw focus point, but much better aligned with the actual cached extract units.
- Startup now clears abandoned `running` jobs once, before the in-memory active-job flag is seeded. That prevents old crashed imports from blocking new focused-road work indefinitely.
