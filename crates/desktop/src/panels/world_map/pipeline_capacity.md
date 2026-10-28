# pipeline_capacity.rs

Shared capacity palette, legend and hover wording for both pipeline renderers.
The optional mode is a static attribute visualization, never flow or utilization.

## Contracts
- `color` applies fixed gas (bcm/y of gas) and oil/liquids (boe/d) bands. Bands
  do not renormalize with zoom, source selection or visible route count.
- Known zero is lavender, missing/invalid/unscaled units are gray. Product/unit
  mismatches never enter the other scale. Hover retains unscaled source units.
- Status dimming is preserved; filtering remains the existing `Filter`'s job.
- Product mode keeps the caller's existing theme colors. Capacity colors are
  shared with the legend and baked into local/globe batches, not animated.
- Source values keep full display precision; no conversion between volume,
  energy, oil-equivalent or time units is inferred.

Tests cover unit mismatches, unknown versus explicit zero, band boundaries,
product-mode restoration and planned-status filtering/dimming.

`map_label` keeps a compact capacity/not-actual-flow caption visible with the drawer closed.
