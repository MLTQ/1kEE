# zoom.rs

## Purpose
Defines terrain detail tiers and the Earth core selection envelope. Original
half extents remain address/sampling parameters, not new stored footprints.

## Components
- spec_for_zoom retains historical keys, contour intervals and sampling scales.
- prefetch_radius_for_zoom covers the oblique viewport with disjoint cores and
  one spare hosted ring; source downloads/processing remain separately bounded.
- region_coverage_half_extent_deg subtracts the worst-case half-core focus offset.
- per_asset_feature_budget preserves the prior deep-tier detail floor.
- Lunar/Mars specs and GeoBounds remain legacy callers' geometry helpers.

## Contracts
Earth step remains half_extent * 0.45. CoreTile derives shared ownership and
smaller padded rasters. The local reader retains up to 16 rings, matching the
region-query limit. Changing the step would require cache migration.
