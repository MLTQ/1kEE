# node_store.rs

## Purpose
Provides the disk-backed candidate-node cache for the offline OSM builder. It exists so large focused exports do not need to keep millions of nodes in RAM between the node pass and the way pass.

## Components

### `NodeStore`
- **Does**: Owns a SQLite database of candidate nodes for one focused build request
- **Interacts with**: `roads.rs`, `rusqlite`

### `NodeStore::reset`
- **Does**: Clears any previous node cache state before a fresh first pass
- **Interacts with**: the `candidate_nodes` and `build_state` tables

### `NodeStore::insert_batch`
- **Does**: Persists candidate nodes in transactional batches during the node scan
- **Interacts with**: `roads.rs`

### `NodeStore::points_for_refs`
- **Does**: Resolves way node references back into points during the second pass
- **Interacts with**: `roads.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `roads.rs` | a completed store can be reopened and queried without loading all nodes into memory | Changing table schema or completion semantics |
| future resumed builds | incomplete stores can be discarded and rebuilt safely | Reusing partial state without validation |

## Notes
- This is intentionally conservative: it optimizes for bounded memory first, not absolute maximum throughput.
- The store lives under `.builder_state/` next to the output cache so it travels with the rest of the offline build artifacts.
