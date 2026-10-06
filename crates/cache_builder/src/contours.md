# contours.rs

## Purpose
Offline Earth SRTM contour construction through GDAL or native marching squares,
with compatible SQLite tile/manifests and shared import support for Mars.

## Components
- all_specs retains Earth buckets 0–6 and their address parameters. Every tier
  uses native one-arc-second SRTM posts; wide views reduce elevation-plane
  density (200/100/50/25/10/5/5 m) without simplifying retained line shapes.
- build_contour_tiles/build_contour_tiles_native plan existing addresses, skip
  ready tiles and generate only each owned core plus a two-pixel halo.
- import_tile_clipped/write_tile_native clip before committing contours and
  coastlines; empty results are known-empty manifests, errors roll back.
- import_tile retains unmodified import behavior for non-Earth callers.

## Contracts
- CoreTile bounds and f64 source grids are shared with desktop generation.
- Both engines use the shared globally aligned `CoreTile::srtm` source grid,
  ensuring common elevation planes have the same source samples across tiers.
- The default database is `srtm_native_v1.sqlite`, separate from old coarse
  raster data. Existing legacy files are not migrated, replaced or deleted.
- Both build entrypoints require the shared native quality tag before planning.
  Empty caches are tagged; untagged nonempty caches are rejected with a request
  for a new destination. This also protects explicitly supplied CLI paths.
- Existing caches are preserved and skipped. This does not compact old data.
- Published progress footprints are owned cores, not padded generation windows.
- GUI Build All still builds SRTM tiers; this change adds no nationwide hosted
  scheduler and starts no background conversion by itself.
- See contour_core_tests.rs for native/GDAL seam and persistence checks.
