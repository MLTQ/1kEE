# lod.rs

## Purpose
Choose an existing terrain source grid from the continuous camera footprint.
The address grids and cached files remain compatible.

## Contracts
- Prefer at most 81 requests; retain a selected tier up to 121 for hysteresis.
- At the widest view the coarsest existing grid needs 225 requests. Coverage
  is preserved instead of clipping the map to meet the normal budget.
- Camera projection never uses the selected source zoom.
- Earth, Moon and Mars share selection but retain their own source tiers.

## Validation
Sweep both directions across every hundredth of camera zoom; verify coverage
and request limits. Pin the event-follow arrival at 81 instead of 529 tiles.
