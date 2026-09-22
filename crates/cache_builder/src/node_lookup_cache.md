# node_lookup_cache.rs

## Purpose
Reuses node-store blocks across ways in a Rayon task, avoiding repeated 64 KiB
reads and allocations for nearby/shared references during the whole-planet scan.

## Components
- `LookupSession`: holds 64 directly mapped blocks (at most 4 MiB of node bytes)
  and reusable query scratch storage. Resolves each batch in input order.
- Cache slots are keyed by absolute record offset. A collision reuses its byte
  buffer for the new block. Failed reads return errors and are never cached as
  valid data; a missing node ID still returns `None` in its original slot.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| Planet Pass 2 | Per-task cache, no shared mutex, exact reference order and f32 coordinates | Unbounded storage or reordered geometry |
| Node store | File stays immutable for the lookup lifetime | Reusing blocks while Pass 1 mutates the file |

## Validation
Compares cached and uncached results including duplicates, absent IDs, block
boundaries, tail blocks, and collisions. Repeated-way checks assert disk blocks
are read only once while resident and cached bytes remain bounded.
Truncation/recovery checks ensure failed blocks cannot silently erase geometry
or poison later successful reads.

An ignored real-data benchmark reads up to 10,000 ways starting at an explicitly
supplied PBF blob offset, opens the matching node store read-only, and alternates
uncached/cached lookups with exact result comparisons. It reports index startup
separately, builds/reopens a sparse index in a unique disposable temporary
directory, and removes it afterward. It never edits the existing source, node
store, checkpoint, or runtime cache files.
