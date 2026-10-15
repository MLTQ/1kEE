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
  not count. Empty decoded tiles and deliberately culled screen cells are valid.
  The spare prefetch ring is excluded.
- `base_radius` translates the camera footprint onto the SRTM address grid;
  maximum zoom needs only nine base tiles.
- `choose` keeps the base's loading snapshot when fallback is needed, and
  attaches the requested source tier and its ready tiles as `detail`. The
  handoff compositor clips base contours to areas without ready detail; a
  missing edge cell cannot demote still-visible fine terrain. Empty decoded
  detail owns its region too; unavailable cells do not.
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
- Fallback availability never changes requested LOD priority. A real zoom-out
  still supersedes finer tiles as the requested coarse cores become ready.
- Supplemental detail carries stable tile Arcs only; merging and ownership
  clipping remain on the existing background composition worker.

## Validation
Companion tests cover missing versus empty tiles, publication, panning, all
deep zoom footprints, source-grid identity, and a temporary-database cold start
followed by an offline fine-cache upgrade.

The scene supplies source zoom separately from camera zoom. Fine/base coverage
and the base envelope use the actual camera footprint; a coarser selected source
never changes screen scale or expands fallback requests to its nominal camera.

The local scene uses the same composed picture for CPU contours and GPU batches.
Legacy transition callers can still use the selected tier's merged `contours`.
