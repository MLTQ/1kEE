# raster.rs

## Purpose
Normalize provider rasters to exact shared contour bounds, dimensions, CRS and
nodata semantics using the installed GDAL tools.

## Contracts
Warp uses Float32 EPSG:4326, bilinear interpolation, -999999 nodata, a 32 MiB
working/cache budget, one processing thread, bounded HTTP retries and the
existing cancellable process runner. An all-nodata raster is not published as
an empty fine tile: the map must retain coarse fallback there. Validation scans
a temporary raw band of at most 2400×2400 Float32 values.
LINZ uses LIBERTIFF (GDAL 3.11+) because this machine's GTiff lacks LERC support.
GSI arrays use an explicit little-endian, north-up VRT. NZ coordinate selection
transforms nine points locally with PROJ networking disabled, observes the same
deadline/shutdown flag, and preserves numeric stdout even with profiling enabled.

The enclosing acquisition deadline bounds each external warp/validation stage.

Exact (`-et 0`) reprojection keeps adjacent core halos sampled consistently
in projected Swiss/NZ terrain.
