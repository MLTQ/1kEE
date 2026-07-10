# geojson.rs

## Purpose
Owns the offline vector-cell cache writer and merge path. It writes desktop-compatible
binary `.1kc` cells and can read legacy GeoJSON only as a migration fallback.

## Components

### `ensure_cache_dir` / `vector_cell_path`
- **Does**: Resolves and creates the output directory plus stable per-cell filenames
- **Interacts with**: `roads.rs`

### `merge_write_cells`
- **Does**: Merges newly built road polylines into existing per-cell `.1kc` files
- **Interacts with**: `roads.rs`
- **Rationale**: Re-running the builder for overlapping bounds should extend a cell cache, not replace it destructively. Existing binary features remain in
  their encoded form so baked elevation arrays are not sampled again on every
  incremental flush.

### `merge_write_feature_cells`
- **Does**: Applies the same ID-keyed merge behavior to non-road feature cells.
- **Interacts with**: `planet_all.rs`, `roads.rs`, `cell_format`.

### `load_*_for_merge` / `ensure_elevations`
- **Does**: Retains existing `CellFeature::elevations` when a binary cell is
  revisited, and samples only incoming or legacy features that lack them.
- **Interacts with**: `SrtmSampler`, `cell_format::read_single_chunk`.
- **Rationale**: Removing elevation fields would change cache compatibility;
  preserving them avoids wasted terrain sampling without changing geometry or
  the field schema.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `roads.rs` / `planet_all.rs` | can write feature groups directly into desktop-compatible cell caches | Changing file naming or merge-by-ID behavior |
| desktop cell loaders | cache files use the `.1kc` schema, including optional elevations | Renaming fields or changing geometry shape |

## Notes
- Existing GeoJSON files remain readable as migration input; fresh output is
  `.1kc`.
- This layer intentionally writes every checkpointed batch durably. Deferring
  arbitrary dirty cells requires a durable delta journal to keep resume offsets
  safe, rather than an unbounded in-memory map.
