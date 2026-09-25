# contour_fallback.rs

## Purpose
Keep SRTM terrain available at deep camera zoom wherever the requested USGS
contour tier is missing, uncovered, offline, or still building. Camera scale,
road detail, and the Moon/Mars pipelines remain independent of this choice.

## Components
- `load` requests the normal tier, then the finest SRTM tier if it does not yet
  cover the view. Both paths use the existing asynchronous manifest/read/merge
  pipeline. The SRTM cache is shared with normal bucket-six viewing and survives
  transitions between hosted tiers, so fallback works on cold starts and pans.
- `covers_view` requires actual decoded and published tiles for every core
  intersecting the padded oblique viewport. Known-uncovered progress cells do
  not count. Empty decoded tiles are valid. The spare prefetch ring is excluded.
- `base_radius` translates the camera footprint onto the SRTM address grid;
  maximum zoom needs only nine base tiles.
- `choose` displays one complete source tier and its matching loading snapshot.
  Available SRTM stays visible until fine coverage is ready across the view;
  if no base geometry exists, partial fine arrivals remain visible. This avoids
  doubled contours from simultaneously drawing SRTM and USGS heights.
- `pending` records work for the current request, so an inactive fallback cache
  with an old incomplete status cannot keep the repaint loop running forever.

## Contracts and limits
- Persisted fine tiles always get a chance to load, including with streaming
  disabled; a coverage probe cannot hide offline data.
- No filesystem work or full geometry copies happen on paint. Base and primary
  each retain the existing bounded readers and generation checks.
- `LocalContourLoad.source_zoom/build_radius` describe the selected loading
  grid. Projection continues using the actual camera zoom.
- The base is existing bucket six (5 m contour spacing from approximately 30 m
  SRTM samples), with no per-tile path-count thinning. Zoom does not invent
  higher-resolution measurements. Areas without SRTM or cached terrain still
  need another source. Cache formats and persisted data are unchanged.
- A view crossing the fine-coverage boundary stays on SRTM as a whole. Mixing
  tiers spatially would require clipping ownership boundaries to avoid double
  contours. This policy favors continuous terrain across that view.

## Validation
Companion tests cover missing versus empty tiles, publication, panning, all
deep zoom footprints, source-grid identity, and a temporary-database cold start
followed by an offline fine-cache upgrade.
