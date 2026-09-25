# mod.rs

## Purpose
Route small, on-demand bare-earth elevation windows to official international
providers, for the existing contour and point-sampling pipelines.

## Components
- `Provider` holds geographic screens, visible credits, and official links.
  Screens only nominate candidates; actual raster availability decides coverage.
- `fetch` tries overlapping candidates, keeps network errors distinct from
  confirmed absence, and atomically publishes one normalized Float32 raster.
- `Scratch` removes every temporary raster, VRT, and partial download on all exits.
- `Permit` limits each provider to two active jobs, shared by contours and samples.

## Contracts
Only background workers call fetch. Requests are finite, at most 0.25 degrees
per axis and 2400 pixels per side. GDAL subprocesses have timeout/shutdown
handling. Current coverage is England, metropolitan France, Netherlands,
Switzerland, New Zealand's main islands and Japan. USGS remains its own adapter.
Completed contours persist in the existing cache; source rasters do not.

A per-request 180-second deadline covers catalog pagination, PNG fallbacks,
provider admission and raster work. Individual HTTP requests share the remaining
budget, including waits for a provider slot; shutdown and deadline failures stay retryable.
