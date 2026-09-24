# contour_clip.rs

## Purpose
Clip generated GeoPackage contour lines before durable insertion. Retains f64
coordinates and preserves each feature's elevation in the caller.

## Components
- clip_gpkg validates XY line input, clips parts and omits empty results.
- encode emits a little-endian EPSG:4326 XY MultiLineString.

## Contracts
- Reject unsupported, truncated or nonfinite geometry; callers roll back imports.
- Mixed child endian is supported. Counts are bounded before allocation.
- The resulting blobs work with existing SQLite and .1ka readers.
