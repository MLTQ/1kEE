# planet_all.rs

## Purpose

Builds global `.1kc` vector-cell caches from a planet PBF in two resumable
passes. Pass 1 writes and sorts the node store; Pass 2 resolves way geometry in
parallel and incrementally merges feature cells.

## Components

### `Checkpoint`

- **Does**: Records durable Pass 1 and Pass 2 restart positions plus node-count
  state in a small checkpoint file.
- **Interacts with**: `NodeWriter`, `open_planet_at`, `build_planet_cache_with_progress`.
- **Rationale**: Checkpoint replacement is atomic so an interrupted write keeps
  the last complete restart state instead of leaving a partial text file.

### `run_pass1`

- **Does**: Streams all PBF nodes to the flat node file, periodically makes the
  file durable, then advances the checkpoint at blob boundaries. A resumed pass
  truncates the node file to `pass1_record_count` before replaying source data.
- **Interacts with**: `flat_node_store.rs`, `roads.rs` position reader.

### `process_way`

- **Does**: Classifies an OSM way and emits selected vector features into the
  affected 1-degree cells.
- **Interacts with**: `NodeLookup::lookup_many`, `util.rs` classifiers.
- **Rationale**: Resolving every reference in one grouped lookup amortizes
  positional I/O without changing vertex order, omissions, or `f32` values.

### `run_pass2` / `flush_all`

- **Does**: Decodes blobs in parallel, merges accumulated feature maps, writes
  cells incrementally, and advances its restart offset only after the flush.
- **Interacts with**: `geojson.rs`, `srtm.rs`.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| CLI and builder GUI | `build_planet_cache_with_progress` returns a readable summary and progress lifecycle | Changing public result/progress behavior |
| resumed builds | `pass1_offset` and `pass2_offset` are start-of-next-blob offsets; Pass 1 resumes from its saved record count, not trailing file length | Saving a mid-blob/pre-durability offset or retaining an uncheckpointed node-file tail |
| cache readers | Emitted `.1kc` geometry, feature metadata, and class encoding remain compatible | Changing feature selection, vertices, or cell schema |
| `flat_node_store.rs` | Pass 2 calls lookups only after Pass 1 sorted output is complete | Skipping sort completion state |

## Notes

- Incremental cell merging makes repeated data at a resume boundary harmless:
  writers replace each feature by its stable OSM way ID.
- SRTM baking, when requested, is delegated to the cell merge layer so one
  sampler lives for the full Pass 2 run.
