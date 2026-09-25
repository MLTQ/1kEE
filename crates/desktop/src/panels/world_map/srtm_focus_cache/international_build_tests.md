# international_build_tests.rs

## Purpose
Exercise the complete hosted-source routing, bounded queue, GSI acquisition,
GDAL contouring, core clipping, SQLite publication, and temporary-file cleanup.

## Contracts
The ignored live test requests one maximum-zoom tile at Tokyo station. It uses
a uniquely owned temporary cache and verifies nonempty persisted contours and
the absence of temporary source rasters after success. The shared staging
directory may remain empty, because other workers can use it. It never touches the
user's cache. Run explicitly with network access and GDAL installed.
