# projection.rs

## Purpose
Holds the globe projection math shared across geography and marker rendering. It converts lat/lon samples into screen-space points and splits path drawing by front-facing versus back-facing segments.

## Components

### `project_geo`
- **Does**: Projects one geographic point onto the globe using the current view and optional terrain exaggeration
- **Interacts with**: `GlobeLayout` and `ProjectedPoint` in `mod.rs`

### `project_geo_elevated`
- **Does**: Projects a point with extra radius above the terrain surface for beam-tip placement
- **Interacts with**: `draw_event_marker` in `markers.rs`

### `draw_geo_path` / `flush_segments`
- **Does**: Paint front-facing and back-facing geographic line segments with different stroke treatment
- **Interacts with**: `geography.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `geography.rs` | Front/back hemisphere handling stays stable for long paths | Removing segment splitting or changing alpha semantics |
| `markers.rs` | Projected marker positions align with the rendered globe | Changing projection handedness or coordinate basis |
