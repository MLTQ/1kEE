# contour_loading.rs

## Purpose
Combines background build measurements, contour row decoding, and publication
into the local scene's progress snapshot without filesystem work on paint.

## Components
- `Snapshot` carries ready buckets, per-tile fractions, and region counters.
- `snapshot`: keeps known no-source buckets complete, maps persisted tiles to
  75%, advances with decoded row counts to 99%, and completes only once decoded
  geometry is published in the current merge (or a decoded tile is empty).
- Existing ready/total counts now include in-memory loading and publication.
  The render cache's epoch checks reject late readers and merges after resets.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| `contour_asset.rs` | Short cache lock and atomic reads only | Disk access, new workers, or copying geometry on paint |
| Loading grid | A disk manifest is not the same as completed loading | Hiding cells before decode/publication |
| Root/zoom changes | Keys include full asset path and zoom; clears remove publication state | Reusing old published tiles in a new scene |
