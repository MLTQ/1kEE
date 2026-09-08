# mod.rs

## Purpose

Coordinates terrain contour cache discovery, read-time region selection, and
on-demand Earth, lunar, and Mars tile builds.

## Components

### `ensure_*_contour_region`

- **Does**: Returns ready tile assets for the visible region and schedules
  missing tiles through the builder subsystem. Existing databases are queried
  read-only; only a missing database is opened writable for first-run schema
  initialization. Callers provide separate prefetch and build radii; local
  terrain intentionally uses its complete wide envelope for both so an
  overlapping off-center owner cannot leave a visible gap. Tiles are visited
  center-first.
- **Interacts with**: `db.rs`, `builders.rs`, and `contour_asset.rs`.

### Region/status helpers

- **Does**: Report ready tiles and build progress without mutating cache files.
  Local loaders can derive the same information directly from their selected
  manifest snapshot instead of issuing a second region query. `ready_buckets`
  is the set of buckets that need no loading treatment; the status counters
  describe only buckets that can hold contours.
- **Interacts with**: map overlays and local/globe renderers.

### Cache path helpers

- **Does**: Resolve Earth, lunar, and Mars SQLite cache paths beneath the
  selected derived-data root.
- **Interacts with**: `terrain_assets.rs` and `db.rs`.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| `contour_asset.rs` | Existing cached tiles are discoverable without requiring write access | Using writable cache setup for renderer lookups |
| `builders.rs` | Missing tiles can use the writable schema-initializing connection | Removing builder-side write access |
| Map UI | Region queries are cheap enough for repeated frames | Synchronous GDAL work on the UI thread |

## Notes

- Renderer/status lookups intentionally use `db::open_cache_db_read_only`; this
  keeps existing contours available while a full cache volume prevents builds.
- `open_region_cache_db` preserves first-run builder behavior without putting
  established render-time caches back through writable WAL/schema setup.
- Region selection reads each manifest rectangle with one indexed query, then
  reuses that snapshot for all tile decisions. Lunar and Mars helpers honor
  the same prefetch/build radii as Earth; globe callers explicitly retain their
  smaller equal-radius requests.
- Failed tile builds enter a bounded exponential per-tile cooldown, preventing
  an unavailable source or full cache volume from repeatedly consuming
  background build slots.
- Earth buckets over open ocean have no SRTM source file, so they can never
  produce contours. They join `ready_buckets` — stopping the local pulse grid —
  but are excluded from `total_assets`, so the cache card counts only terrain
  that is genuinely outstanding rather than stalling short of its total.
- Latitude coverage and ocean coverage are separate exclusions: the former is a
  static per-body bound (lunar/Mars), the latter is discovered per bucket from
  the SRTM source layout.
- Earth zoom buckets 7 and above source USGS 3DEP 1 m rather than SRTM, because
  SRTM's ~30 m posting cannot support their sub-5 m intervals. Those buckets
  need no local source root; they are gated on published 3DEP coverage instead,
  and a bucket with no 1 m source joins `ready_buckets` for the same reason an
  ocean bucket does. `FocusContourSpec::interval_m` is `f32` so those tiers can
  ask for sub-metre intervals.
- 3DEP tiles cost a network request each, so `prefetch_radius_for_zoom` tightens
  their envelope to a 5×5 grid instead of the local SRTM tiers' 13×13.
