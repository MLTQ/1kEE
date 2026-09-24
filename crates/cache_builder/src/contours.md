# contours.rs

## Purpose
Offline Earth SRTM contour construction through GDAL or native marching squares,
with compatible SQLite tile/manifests and shared import support for Mars.

## Components
- all_specs retains Earth buckets 0–6 and original address/sampling parameters.
- build_contour_tiles/build_contour_tiles_native plan existing addresses, skip
  ready tiles and generate only each owned core plus a two-pixel halo.
- import_tile_clipped/write_tile_native clip before committing contours and
  coastlines; empty results are known-empty manifests, errors roll back.
- import_tile retains unmodified import behavior for non-Earth callers.

## Contracts
- CoreTile bounds and f64 source grids are shared with desktop generation.
- Both engines keep the original elevation intervals and approximately the same
  sample spacing; source pixel counts shrink by approximately 95% per address.
- Existing caches are preserved and skipped. This does not compact old data.
- Published progress footprints are owned cores, not padded generation windows.
- GUI Build All still builds SRTM tiers; this change adds no nationwide hosted
  scheduler and starts no background conversion by itself.
- See contour_core_tests.rs for native/GDAL seam and persistence checks.
