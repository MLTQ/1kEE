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

### Sourceless tile memo

- **Does**: Records Earth buckets whose footprint contains no SRTM source file
  at all, so they are never scheduled, never enter a failure cooldown, and are
  reported to region state as resolved terrain.
- **Interacts with**: `gdal::bounds_have_srtm_source`, `mod.rs` region state,
  and the local pulse grid.
- **Rationale**: SRTM ships land cells only. An open-ocean bucket previously
  looked identical to a failed build: it retried on a cooldown forever, never
  gained a manifest row, and the dissolve pulse replayed over open water
  indefinitely while consuming build slots owed to real land tiles.

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
| Local pulse grid | Buckets with no source terrain are reported once and stay reported for the active source root | Dropping the memo per repaint, or keying it without the root |

## Notes

- `allow_build=false` is intentionally a no-op on cache misses; the tile will
  be scheduled when it enters a caller's build window or a narrower caller
  requests it.
- Desktop on-demand GDAL generation is capped at two concurrent jobs. This
  leaves capacity for egui, WGPU, SQLite reads, and merge workers while the
  local 13×13 envelope fills; the companion cache builder is the preferred
  path for high-throughput bulk precomputation.
- Builder calls receive the region selector's manifest count rather than
  querying SQLite per tile. Completed in-process builds advance a revision so
  local manifest snapshots refresh immediately.
- The sourceless memo is scoped to the source root that produced it and is
  cleared by a manual cache reset, so a remounted volume or a newly picked data
  root re-checks every bucket rather than inheriting stale ocean verdicts. It
  is bounded and rebuilt on overflow; re-checking is a few `exists()` calls.
- Only Earth needs the memo. The lunar and Mars sources are global rasters, so
  their in-latitude buckets always have source coverage.
- Mars records its uncovered buckets with `db::mark_tile_empty`; Earth
  intentionally does not. A Mars bucket with no CTX and no MOLA is permanently
  uncovered by the dataset, whereas a missing SRTM cell can also mean a partial
  download or a terrain volume that was not mounted when the bucket was first
  visited. Keeping the Earth verdict in memory makes that mistake cost one app
  restart instead of a poisoned on-disk manifest.
- Failed GDAL/cache writes enter a bounded exponential per-tile cooldown. This
  prevents a full cache volume or missing source file from repeatedly consuming
  all build slots; a per-body gate also pauses the next outer-ring batch after
  a failure. A successful build or manual cache reset clears the gate, while a
  manifest hit clears the matching tile's cooldown.

## 3DEP routing

`ensure_bucket_asset` dispatches on `zoom::spec_uses_threedep`. SRTM buckets keep
their existing source-root requirement and ocean memo; 3DEP buckets skip both and
are gated on `threedep::coverage_at` instead. A resolved negative probe is
recorded in a separate uncovered-tile memo — kept apart from the SRTM one, which
is invalidated per source root, because 3DEP coverage is a property of the
service rather than a local directory. An unprobed bucket stays outstanding so it
is retried once its probe lands.
