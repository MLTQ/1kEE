# mod.rs

## Purpose
Coordinate optional, offline SWOT_06 marine vertical gravity gradient imagery
and expose its controls without loading data on the UI thread.

## Components
- `texture` uses independent single-flight globe/local snapshots keyed by data
  root and local pixel window. Workers resolve paths, validate/read grids,
  convert colors and queue texture upload. Errors are retained until a root
  change, terrain cache reset or layer off/on cycle.
- `controls` owns the Earth-only toggle, load/error status, fixed ±20 Eötvös
  legend. Attribution stays in `docs/swot-marine-gravity.md` and the data manifest;
  the layer menu contains no reload button or citation links.
- `clear` releases both texture snapshots and the globe projection mesh;
  single-flight epochs reject late results after reset or disable.

## Contracts
Data lives under the configured/discovered `Derived/swot` directory. The
importer prepares both required BIL grids. No API key or runtime network is
needed. The layer starts off, uses session visibility, and never supplies
elevation or replaces GEBCO. `globe`/`local` own projection only; `grid` owns
the binary and palette contracts. The drawer does no filesystem work.
