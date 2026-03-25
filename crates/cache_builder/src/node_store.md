# node_store.rs

## Purpose
Provides the disk-backed candidate-node cache for the offline OSM builder. It exists so large focused exports do not need to keep millions of nodes in RAM between the node pass and the way pass. Also stores byte-offset checkpoints for both scan passes so a killed process can resume exactly where it left off.

## Components

### `NodeStore`
- **Does**: Owns a SQLite database of candidate nodes and build-state metadata for one focused build request
- **Interacts with**: `roads.rs`, `rusqlite`

### `NodeStore::reset`
- **Does**: Clears any previous node cache state before a *fresh* first pass — not called when resuming
- **Interacts with**: the `candidate_nodes` and `build_state` tables

### `NodeStore::insert_batch`
- **Does**: Persists candidate nodes in transactional batches during the node scan; uses `ON CONFLICT DO UPDATE` so re-inserted nodes from a resumed scan are idempotent
- **Interacts with**: `roads.rs`

### `NodeStore::points_for_refs`
- **Does**: Resolves way node references back into coordinates during Pass 2; uses a single `WHERE id IN (…)` query per chunk of 999 IDs instead of one round-trip per node (10-50× faster for long ways)
- **Interacts with**: `roads.rs`

### `NodeStore::save_scan_offset` / `get_scan_offset` / `clear_scan_offset`
- **Does**: Persists and retrieves a named file-byte-offset checkpoint (keyed `"node_scan"` or `"way_scan"`) in the `build_state` table so interrupted scans can resume at the exact next-blob boundary
- **Interacts with**: `roads.rs`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `roads.rs` | a completed store can be reopened and queried without loading all nodes into memory | Changing table schema or completion semantics |
| resumed builds | `insert_batch` is idempotent (ON CONFLICT DO UPDATE), `points_for_refs` works on partial stores | Breaking upsert semantics |
| `save_scan_offset` callers | offset is the file position of the *next unread blob*, so seeking to it on restart is immediately correct | Storing mid-blob offsets |

## Notes
- Scan offsets are cleared (`clear_scan_offset`) only after the corresponding pass completes fully; a partial offset surviving a crash is intentional and enables resumption.
- `points_for_refs` chunks at 999 to stay within SQLite's default `SQLITE_LIMIT_VARIABLE_NUMBER`; if that limit is raised in a custom build, the chunk size can be increased proportionally.
