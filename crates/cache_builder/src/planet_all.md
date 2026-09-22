# planet_all.rs

## Purpose

Builds global `.1kc` vector-cell caches from a planet PBF in two resumable
passes. Default Pass 1 writes and sorts the node store; opt-in compact builds
delegate indexing to `planet_compact.rs`. Both resolve way geometry in the shared
parallel Pass 2 and incrementally merge feature cells.

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
- Decodes up to 64 blobs in parallel through `planet_scan.rs`, then appends the
  encoded records in source order before checkpointing the batch's final offset.
- **Interacts with**: `flat_node_store.rs`, `roads.rs` position reader.

### `process_way`

- **Does**: Classifies an OSM way and emits selected vector features into the
  affected 1-degree cells.
- **Interacts with**: `NodeLookup::lookup_many`, `util.rs` classifiers.
- Uses the `planet_lookup` adapter for grouped coordinate queries: flat sessions
  cache up to 4 MiB and compact sessions up to 16 MiB of decoded node arrays.
  Reference order and coordinate values are identical across backends.
- Compact sessions prepare each way blob's dependencies together before feature
  classification to avoid repeatedly decompressing evicted node blocks.
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
- Pass 2 propagates PBF decode errors instead of silently checkpointing past
  unreadable blobs. Its node-block cache is task-local and needs no shared lock.
- A synthetic PBF regression verifies that resumed parallel Pass 1 truncates
  an uncheckpointed node tail and retains the exact original records/EOF offset.
- Pass 2 announces node-index startup before opening it and reuses the validated
  `planet_nodes.sparse-index-v1` scratch sidecar on subsequent starts. Existing
  node/checkpoint files remain compatible and are not rebuilt for this change.
- A corrupt-way fixture verifies that a failed Pass 2 leaves its saved checkpoint
  unchanged, rather than recording an unreadable blob as completed work.
- Node-store I/O failures propagate with way ID and byte offset, stopping before
  output/checkpoint advancement rather than emitting incomplete geometry.
- Compact mode validates original-source identity around batches and before
  checkpoint advancement. Its isolated state never reads legacy checkpoints.
