# roads.rs

## Purpose

Builds focused offline vector caches from an OSM planet PBF. Pass 1 stores
candidate nodes in SQLite; Pass 2 reconstructs selected roads, waterways,
buildings, tree cover, and infrastructure features into per-layer `.1kc` cells.
Both passes are resumable at PBF blob boundaries.

## Components

### `PosReader` / `open_planet_at`

- **Does**: Wraps a seekable planet file and records the byte offset of the
  next PBF blob consumed by `BlobReader`.
- **Interacts with**: `planet_all.rs`, both focused-build passes.
- **Rationale**: A blob boundary is a safe exact restart point.

### `build_bbox_cache_with_progress`

- **Does**: Validates the focused request, builds or resumes the node store,
  exports enabled feature layers, and emits GUI-safe progress updates.
- **Interacts with**: `node_store.rs`, `geojson.rs`, `admin.rs`, `app.rs`.

### `collect_candidate_nodes`

- **Does**: Retains in-bounds nodes, committing their upserts and a
  `"node_scan"` checkpoint in one transaction at a 50k retained-node batch or
  one million scanned nodes.
- **Interacts with**: `NodeStore::insert_batch_and_save_scan_offset`.
- **Rationale**: Dense scans retain efficient batches while sparse regions have
  bounded replay work.

### `collect_all_features_by_cell` / `flush_all_chunks`

- **Does**: Rebuilds ways from the candidate-node map, classifies enabled
  vector layers, and incrementally writes binary `.1kc` cells after every
  100k buffered features.
- **Interacts with**: `geojson.rs`, `srtm.rs`, `util.rs` classifiers.
- **Rationale**: Checkpointed output remains usable after interruption and
  duplicate OSM way IDs are safely replaced on resume.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `main.rs` / `job.rs` | readable `Result` failures and monotonic `RoadBuildProgress` updates | Changing result or progress contracts |
| resumed node scans | candidate rows and their next-blob offset commit together | Advancing an offset separately from rows |
| resumed way scans | every completed cell flush precedes the saved `way_scan` offset | Saving an offset before its `.1kc` output is durable |
| desktop cell loaders | per-layer `.1kc` files retain feature IDs, classes, names, points, polygon flags, and optional elevation fields | Changing the cell schema or geometry encoding |

## Notes

- The focused path supports roads, waterways, buildings, forests, power,
  rail, pipelines, aeroways, military, communications, industrial, ports,
  government, and surveillance layers. Optional admin boundaries are delegated
  to `admin.rs`.
- Existing legacy GeoJSON files are migration input only; fresh output is
  binary `.1kc`.
- Cross-checkpoint write coalescing requires a durable delta journal or spill
  layer; retaining all dirty cells only in memory would break planet-scale
  memory bounds and resume safety.
