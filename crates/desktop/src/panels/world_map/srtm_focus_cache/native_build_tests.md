# native_build_tests.rs

## Purpose
Verify the production desktop GDAL pipeline retains native SRTM geometry even
at the widest globe source tier. All outputs use an isolated temporary directory.

## Coverage
- Builds Yemen core 0/9/27 from the read-only source root in ONEKEE_SRTM_ROOT.
- Checks the native quality tag, owned bounds, selected elevation planes, a
  >5800-pixel raster and median contour segment spacing at native resolution.
- Reports geometry counts and build time; ONEKEE_NATIVE_OUTPUT optionally
  exports geometry for visual inspection. Removes temporary build/cache files.

## Contracts
Ignored by default; requires installed GDAL. Does not modify existing terrain
caches, source rasters, running app state or OBS. Build time is not frame rate.
