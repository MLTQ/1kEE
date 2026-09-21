# contour_reader.rs

## Purpose
Loads cached local terrain with two bounded readers on machines with at least
four available CPUs, otherwise one. Each tile becomes available for merging as
soon as it decodes, without waiting behind the rest of an eight-tile batch.

## Components
- `spawn_local_read`: single coordinator per cache, preserving the existing
  batch limit, retry policy, and epoch. Interleaved center-first requests let
  the two nearest tiles start together; each worker reuses a SQLite connection.
- `publish_tile`: accepts a decoded tile only for the current epoch and active
  batch, bumps the merge revision, and removes that tile's reading progress.
  A stale result stops that worker before it reads its next tile.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `contour_asset.rs` | One batch coordinator, at most two readers per local body cache | Unbounded per-tile thread spawning |
| Cache reset/root/zoom changes | Stale workers cannot publish or clear new batch state | Dropping epoch checks |
| Loading grid | Decoded tiles wait at 99% until an accepted merge | Marking nonempty decoded tiles as rendered |

## Notes
The outer coordinator joins all workers before clearing the single-flight
batch. Successfully published tiles survive a later query failure or panic.
No SQLite work or decoding runs under the cache mutex. Globe readers retain
their existing single-reader behavior.
