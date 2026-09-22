# waterway_layer.rs

## Purpose
Loads cached river/stream cells on a background worker and renders their elevated
polylines. Shares archive/binary lookup with other vector layers.

## Contracts
- Baked elevations retained by `cell_loader` take priority; only missing/invalid
  heights call the runtime SRTM sampler. The existing 2m drawing offset remains.
- View bounds/root/generation determine cache lifetime; camera projection stays
  in the draw pass. No new per-frame filesystem work is introduced.
- Full detail and feature geometry are retained by this first archive version.
