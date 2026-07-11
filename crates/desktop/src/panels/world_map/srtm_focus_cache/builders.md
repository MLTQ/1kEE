# builders.rs

## Purpose

Schedules bounded on-demand contour generation for Earth, Moon, and Mars cache
tiles. It keeps source/GDAL work off the render thread while returning existing
SQLite assets immediately.

## Components

### `ensure_*_bucket_asset`

- **Does**: Returns a ready cache asset when present; otherwise, when allowed,
  claims a bounded background build slot and schedules terrain generation.
- **Interacts with**: `mod.rs` region selection, `gdal.rs` builders, and
  `db.rs` tile manifests.
- **Rationale**: Callers can load existing outer tiles without triggering
  speculative writes when their build radius is narrower than their prefetch
  radius. Local terrain currently builds its full source envelope because
  overlapping Moon/Mars geometry can be uniquely owned by an outer tile.

### Pending sets and build slots

- **Does**: Deduplicate active tiles and cap concurrent GDAL work across
  terrain bodies, with a stricter cap for shared lunar/Mars sources.
- **Interacts with**: app shutdown and repaint scheduling.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `mod.rs` | Ready tiles are returned even when `allow_build` is false | Skipping cache manifest checks for prefetch tiles |
| Render thread | Missing tiles never run GDAL synchronously | Doing source work before a worker is spawned |
| Cache builders | At most the configured number of background builds run | Removing pending/slot accounting |

## Notes

- `allow_build=false` is intentionally a no-op on cache misses; the tile will
  be scheduled when it enters a caller's build window or a narrower caller
  requests it.
- Builder calls receive the region selector's manifest count rather than
  querying SQLite per tile. Completed in-process builds advance a revision so
  local manifest snapshots refresh immediately.
- Failed GDAL/cache writes enter a bounded exponential per-tile cooldown. This
  prevents a full cache volume or missing source file from repeatedly consuming
  all build slots; a per-body gate also pauses the next outer-ring batch after
  a failure. A successful build or manual cache reset clears the gate, while a
  manifest hit clears the matching tile's cooldown.
