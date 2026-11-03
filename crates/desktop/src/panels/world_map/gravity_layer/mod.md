# mod.rs

## Purpose
Offer one selected NCEI anomaly quantity/region as an optional Earth overlay in
Globe and Local views, without synchronous disk reads or runtime network calls.

## Components
- `texture` uses one single-flight snapshot keyed by drive, product and GPU
  limit. Incompatible products/roots are never displayed while loading another.
- `clear` resets texture, mesh, error and late-worker epoch state.
- `controls` owns the quantity/region menu and physical-unit legend.
- `render` owns projection calls and the retained globe mesh.
- `grid` owns the prepared-file and color-transfer contracts.

## Contracts
The layer starts hidden and uses prepared regional rasters on Hilbert. A
selection changes neither elevation nor source data. Missing/outside coverage
is transparent. Failures remain cached until selection/root/reset changes.
The single selected quantity prevents overlapping unlike anomaly corrections.
SWOT is independently controlled and remains below this image when both show.
