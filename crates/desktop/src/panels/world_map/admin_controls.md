# admin_controls.rs

## Purpose
Expose nested administrative level checkboxes, region shading, opacity and cache
status beneath the master Admin Boundaries toggle in the layer drawer.

## Contracts
- Children appear only while the parent is enabled; selections survive hiding.
- Country/state/province/county/municipality labels include OSM levels, with a
  tooltip explaining regional naming differences. Intermediate levels stay usable.
- Missing/empty data includes a concrete build/reload instruction. Loading,
  partial coverage, older line-only geometry and parse errors remain observable.
- Parent remains session-only, matching other OSM overlays. No implicit PBF scan.

Opacity uses a separate label so the percentage slider fits the 230-point drawer.

Load from PBF explicitly chooses source and output, then starts the shared importer
in a background thread; Open boundary cache accepts the parent or admin_cells
folder directly. Status includes progress, active external builds and read errors.
Files refresh automatically in Globe and Local; no companion application is required.
