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
- Compact sessions share selected dependencies across bounded groups of blobs
  after classification, avoiding repeated decompression across workers.
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
  unreadable blobs. Its prepared coordinate table is immutable and needs no shared lock.
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

When requested, admin boundaries now run after Pass 2 using the existing node
lookup and dedicated resumable relation/member-way scans. The summary includes
administrative ring counts; admin geometry is no longer skipped.

Admin-only builds bypass general Pass 2 entirely without touching its checkpoint.
`planet_way_selection` classifies once before compact prefetch; unmatched ways
perform no coordinate I/O. Scanned-way counts include rejected ways. Progress
reports source bytes before/after each batch regardless of feature count; a
256 MiB byte cadence flushes pending output before saving sparse-run checkpoints.
The CLI reports the same progress stages as the GUI.

`planet_pass2_tests` checks zero coordinate reads for unmatched ways, genuine
scan counts/progress and normal selected-road output against real PBF fixtures.

## Shared compact passes
Compact Pass 2 now collects up to 256 way blobs (also stopping after 256 MiB of
compressed input), rather than resolving each blob independently. Selected
references are grouped under a memory-aware budget: Linux MemAvailable / 384,
clamped to 4–128 million references; other hosts default to 64 million.
The divisor reserves headroom for coordinate tables, geometry and other apps;
it is not a strict whole-process memory limit. Each subgroup resolves the union
of its node dependencies once. An oversized individual blob fails explicitly
instead of falling back to pathological per-way rereads. Flat mode keeps 64 blobs.
All subgroups must succeed before the existing flush/checkpoint logic advances.
This changes neither geometry nor the existing compact index/checkpoint format.

Operator benchmarks are ignored tests `inspect_operator_way_batch_dependencies`
and `benchmark_operator_shared_coordinates`; they require ONEKEE_BENCH_SOURCE,
ONEKEE_BENCH_INDEX (a disposable copy), and ONEKEE_BENCH_OFFSET. The first estimates
compressed dependency bytes; the second compares exact coordinates and wall time
in old/new/new/old order without dropping the host page cache. Neither writes
output cells. On the stopped Minkowski batch, 64 blobs independently demand
1,164,117,748,654 bytes versus 61,407,831,570 bytes for their union. This is a
read-amplification measurement, not an end-to-end speedup claim. 49 of those
blobs exceeded the old prefetch cap and could reread still more data.

Parallel old/new/new/old sample on 2026-11-08: four blobs, first 128 ways each,
15,527 references. Old: 6.054/6.141 s, 674 block reads (1,227,290,733 bytes).
Shared: 4.314/4.628 s, 649 block reads (1,182,609,238 bytes). All coordinates
matched exactly; combined benchmark peak RSS was 425,428 KiB. These small,
mostly warm-cache samples do not exercise the old 524,288-reference fallback
or establish whole-planet build time. Synthetic tests cover that threshold.
