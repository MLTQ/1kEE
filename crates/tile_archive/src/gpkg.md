# gpkg.rs

## Purpose
Shared GeoPackage XY LineString/MultiLineString decoder extracted from the
desktop contour reader. Used once while packing and by the legacy runtime path.

## Contracts
- Generic point construction avoids a second coordinate-vector conversion.
- Retains existing f64-to-f32 conversion, endian handling, envelope offsets,
  row/part/point order, and malformed-geometry behavior.
- Rejects nested collections and impossible point/part counts before allocating.
- This preserves the existing XY-only renderer interpretation. It is not a
  general replacement for GDAL or a lossless store of original f64 geometry.
- Existing desktop WKB security/order tests exercise these shared functions.
