# swiss.rs

## Purpose
Discover intersecting swissALTI3D COGs through the official STAC bbox query and
read just the requested area via GDAL HTTP range access.

## Contracts
Pagination is followed with cycle/page/source-count bounds; truncation is an
error, not false no-coverage. One newest survey is selected per stable 1 km
cell. Fine views prefer 0.5 m assets; requests at 2 m or coarser prefer 2 m to
reduce I/O. Projected EPSG:2056 rasters are normalized by raster.rs. ZIP/XYZ
assets and unrelated hosts are never passed to GDAL.
