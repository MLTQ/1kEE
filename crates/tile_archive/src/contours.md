# contours.rs

## Purpose
Stores a terrain tile as render-ready f32 coordinate arrays grouped in source
row/part order. GeoPackage decoding and f64 conversion happen during packing.
SQLite fetches one tile payload instead of returning one row per contour.

## Contracts
- CTF1 envelope: magic, u32 row count, repeated f32 elevation + u32 length + bytes.
- Per-row geometry: u32 part count, then per part u32 point count and (lon,lat)
  f32 pairs. Coordinates match the existing renderer, not original f64 precision.
- Full envelope/geometry counts are validated before allocation/publication.
- Source row count and part order preserve progress, sorting and geometry budgets.
- Fingerprints cover database and nonempty WAL metadata. Source changes disable
  packed reads until repacked. Empty WAL creation by a reader does not invalidate.
- Original contour database is still required for tile manifests in this phase.
