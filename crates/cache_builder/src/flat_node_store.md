# flat_node_store.rs

## Purpose

Stores the planet-scale OSM node table as fixed-width records on disk. It keeps
Pass 2 memory bounded while supporting concurrent, read-only coordinate lookups
after an external sort.

## Components

### `NodeWriter`

- **Does**: Streams `(node id, latitude, longitude)` records into the flat file
  and restores an exact durable checkpoint boundary before a resumed append.
- **Interacts with**: `planet_all.rs` Pass 1.
- **Rationale**: Records stay compact (16 bytes); resume truncates any
  OS-flushed tail beyond the checkpoint before replay, preventing duplicate
  node IDs after an interrupted scan.

### `sort_in_place`

- **Does**: Verifies whether records are already ordered and otherwise runs an
  external chunked merge sort by OSM node ID.
- **Interacts with**: `NodeWriter`, `NodeLookup`.
- **Rationale**: Official planet exports are commonly ordered, so the cheap
  pre-check avoids a needless large temporary sort.

### `NodeLookup::lookup_many`

- **Does**: Finds a batch of node coordinates through a sparse in-memory index
  and fixed-size disk blocks.
- **Interacts with**: `planet_all.rs` Pass 2.
- **Rationale**: Batched queries reuse a decoded disk block for all references
  that fall within it, preserving input order while avoiding one allocation and
  `pread` per reference.

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `planet_all.rs` | Records are sorted by signed `i64` node ID before lookup | Changing record layout or sort order |
| `planet_all.rs` resume path | A saved checkpoint never points past durable records, and resumed append discards any later file tail | Advancing checkpoint before `NodeWriter::checkpoint` or appending from full file length |
| Rayon Pass 2 workers | `NodeLookup` can be shared without a cursor race | Removing its read-only / `Sync` behavior |
| `NodeLookup::lookup_many` callers | Results align one-for-one with requested IDs, including `None` for misses | Reordering or dropping result slots |

## Notes

- `FileExt::read_at` maps to positional I/O on supported Unix targets, so it
  does not mutate the shared file cursor.
- A block contains at most 4,096 records; its buffer is reused while processing
  a multi-reference lookup.
