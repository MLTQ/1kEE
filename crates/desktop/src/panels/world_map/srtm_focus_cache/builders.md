# builders.rs

## Purpose
Coordinates background tile generation for the focus cache. It limits concurrency, tracks pending tiles, and launches GDAL build work without blocking the render thread.

## Components

### Build-slot helpers
- **Does**: Limit concurrent contour jobs to a bounded number of worker slots
- **Interacts with**: `ensure_bucket_asset`

### `pending_set` / `is_pending`
- **Does**: Track which tile keys are already in flight
- **Interacts with**: `mod.rs`, loading/progress overlays

### `ensure_bucket_asset`
- **Does**: Return an existing tile immediately or enqueue a background build for a missing tile
- **Interacts with**: `db.rs`, `gdal.rs`, `zoom.rs`
- **Rationale**: Prevents duplicate GDAL jobs while keeping the cache API non-blocking

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `mod.rs` | Missing tiles return `None` and become pending rather than blocking the caller | Making the builder synchronous |
| Loading overlays | Pending-set membership reflects in-flight work accurately enough for UI animation | Removing or delaying pending-state updates |
