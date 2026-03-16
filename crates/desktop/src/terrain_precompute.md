# terrain_precompute.rs

## Purpose
Manages background city-oriented terrain precompute jobs on top of the existing streamed SRTM cache builder. This file exists so the app can queue city cache warming without blocking the UI or inventing a second terrain export pipeline.

## Components

### `PrecomputeJobSnapshot` / `PrecomputeJobState`
- **Does**: Expose per-city progress and lifecycle state for the UI
- **Interacts with**: `terrain_library.rs`

### `queue_city`
- **Does**: Adds a city precompute request if one is not already queued for the same root
- **Interacts with**: `city_catalog.rs`, `terrain_library.rs`

### `tick`
- **Does**: Advances queued jobs by requesting missing SRTM contour neighborhoods for each configured zoom band
- **Interacts with**: `srtm_focus_cache.rs`
- **Rationale**: Reuses the same cache builder as live navigation so precompute and lazy loading stay compatible

### `snapshots`
- **Does**: Aggregates ready/pending totals across the precompute zoom ladder for UI rendering
- **Interacts with**: `terrain_library.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | `tick` is cheap enough to call from the UI loop | Making it blocking or spawning unbounded work per frame |
| `terrain_library.rs` | `snapshots` returns stable per-city progress counts and states | Changing field meanings or progress semantics |

## Notes
- Precompute currently targets a fixed `25 mi` city radius across a shared local zoom ladder.
- Jobs are intentionally additive with lazy loading: precomputed buckets simply become instantly reusable at runtime.
