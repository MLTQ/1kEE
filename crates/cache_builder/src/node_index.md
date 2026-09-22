# node_index.rs

## Purpose
Saves the sparse node lookup index so resuming a planet build avoids millions of
tiny reads against the large node file. The sidecar is disposable build scratch.

## Components
- `stamp`: records node count, file length, modification seconds/nanoseconds,
  device and inode from the opened source file.
- `load`: checks the version, exact size, source stamp, checksum, sorted IDs,
  and expected record stride; any mismatch triggers rebuilding.
- `save`: writes a unique temporary sibling, syncs, and atomically publishes it.
  Failed partial writes are cleaned up; inability to cache does not block a build.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `NodeLookup::open_cached` | Valid index for this immutable file and record count | Reusing stale/corrupt entries |
| Operators | Existing node bytes and checkpoint formats stay unchanged | Making the sidecar authoritative or editing source nodes |

## Notes
This is a local derived cache, not a content-addressed archive. Source mutation
with deliberately restored identity/mtime is outside the immutability contract.
Tests cover valid reuse, every stamp mismatch, corruption, truncation, and cleanup.
