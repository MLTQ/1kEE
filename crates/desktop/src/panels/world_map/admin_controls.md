# admin_controls.rs

## Purpose
Expose nested administrative level checkboxes, region shading, opacity and cache
status beneath the master Admin Boundaries toggle in the layer drawer.

## Contracts
- Children appear only while the parent is enabled; selections survive hiding.
- Country/state/province/county/municipality labels include OSM levels, with a
  tooltip explaining regional naming differences. Intermediate levels stay usable.
- Every level label uses the active map theme's color, making the checkbox list a compact legend
  for outlines and shaded regions. Numeric levels and names remain visible.
- Missing/empty data includes a concrete build/reload instruction. Loading,
  partial coverage, older line-only geometry and parse errors remain observable.
- Parent remains session-only, matching other OSM overlays. No implicit PBF scan.

Boundary opacity is always visible at the top of the enabled section, before the
level list. Its 0–100% slider fades outlines and optional shading together in both
views. Shading opacity remains separately available beneath Shade regions.
Both use separate labels so percentage sliders fit the 230-point drawer.
The shared slider edits native percentage values in one-percent steps, so typing
50% has the same effect as dragging to 50%; the model stores normalized values.

Load from PBF explicitly chooses source and output, then starts the shared importer
in a background thread; Open boundary cache accepts the parent or admin_cells
folder directly. Status includes progress, active external builds and read errors.
Files refresh automatically in Globe and Local; no companion application is required.

Partial caches show published/total regions and explain country-first publication.
Coverage comes from the source watcher, including after an interrupted builder.

Counts describe the loaded camera area and cached eligibility. The drawer observes
the renderer's last published result, so it cannot start a conflicting world load.
