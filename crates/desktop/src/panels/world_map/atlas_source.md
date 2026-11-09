# atlas_source.rs

## Purpose
Read bounded geographic windows from original offline GEBCO, SRTM and cached
fine-elevation rasters. No world-sized height array or new source download.

## Components
- `load` reads original GEBCO at 15 arcseconds, adds local SRTM at 1 arcsecond
  for windows up to 8°, and existing fine-provider chunks below 0.15°.
- `warp` uses the shared cancellable GDAL process runner, one processing thread,
  64 MiB GDAL/warp budgets, a private temporary directory and a 120 s deadline.
  Dateline output columns split into canonical longitude zones without changing
  their source pixel centres. All results retain exact requested pixel edges.
- `merge_land` preserves negative bathymetry beneath SRTM's zero ocean pixels;
  fine nodata never removes a valid coarser source.

## Contracts
Run only on an Atlas background worker. Original sources and their caches are
read-only. Output dimensions honor Region's GPU/memory limits. Fine chunks are
existing normalized provider rasters, and their real availability limits detail.
All intermediate files disappear with the private temporary directory.

EHdr outputs require their sibling .hdr metadata: do not hide directory siblings
with GDAL_DISABLE_READDIR_ON_OPEN=EMPTY_DIR for this operation. The dateline
fixture and real-data tests exercise the actual GDAL output rather than mocks.

`load`/`warp` return descriptive errors rather than erasing GDAL launch, read or
output-dimension failures into None. Atlas displays those errors while retaining
the last valid image; the GDAL path is resolved independently of a terminal PATH.

Before GDAL, detailed windows try the shared read-only Atlas height cache at
Derived/terrain/atlas_heights_v1.sqlite. Matching Land or Terrain tiles supply
measured heights directly. Missing/corrupt coverage falls back to original
sources; available raw SRTM and cached fine-provider terrain retain their higher
resolution when the matching Atlas tier has not been built.

Interactive loads also check atlas_recent_views_v1.sqlite before launching GDAL.
Its key includes every selected source's identity, exact bounds and dimensions.
`Detail::into_raster` queues successful original-source loads for lossless
background persistence. Failed SRTM/fine overlays may display the existing coarse
fallback, but that incomplete result is not retained as a reusable completed view.
Missing, unreadable, full or locked recent-view storage cannot prevent native
terrain from loading. Source identity checks do not hash whole original files.

Saved views can supply both verified heights and prepared pigment bytes directly
to Raster::prepared, skipping color generation as well as GDAL. Height-only
entries are still usable and their regenerated colors are saved asynchronously.
