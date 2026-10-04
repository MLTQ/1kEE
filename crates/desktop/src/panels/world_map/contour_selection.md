# contour_selection.rs

## Purpose
Keep all contour fragments in each Earth local tile's owned core. A path-count
limit favored long interior lines and erased short tile-edge connections,
creating a visible grid in mountainous terrain.

## Components
- `ReadSelection::EarthCore` clips every decoded line to the same ownership
  bounds used by local composition. No feature-count limit applies at any tier.
- `TileSelection::append` clips before vertex simplification, retaining boundary
  intersections, splitting exits and discarding legacy overlapping outer data.
  Only one decoded row's temporary geometry is handled at a time.
- `WholeTile` preserves the existing globe and Moon/Mars budget semantics.
- `finish` preserves the reader's stable absolute-elevation ordering.

## Contracts
SQLite and packed readers share this policy. Earth cores use the compositor's
exact grid; formats, persisted caches and contour intervals do not change.
Worker count, viewport residency and GPU upload limits remain bounded. Full
legacy footprints must not be retained just to eliminate path-count thinning.

Regression and opt-in real-cache checks live in `contour_seam_tests.rs`.
