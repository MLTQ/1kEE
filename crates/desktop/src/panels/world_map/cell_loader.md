# cell_loader.rs

## Purpose
Loads cached vector layers for a view. Prefers smaller packed archive tiles,
then existing binary cells and legacy GeoJSON. Preserves optional baked heights.

## Contracts
- Archive selection happens on background workers, with one connection per load.
- Missing/corrupt/stale archive cells fall back individually.
- Full feature geometry is retained and filtered by bounds. Archive children deduplicate their repeated features; adjacent source-cell fragments retain the existing loader semantics.
- No source PBF scan occurs here. Empty/missing cells remain independent.
