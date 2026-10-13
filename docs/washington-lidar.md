# Washington LiDAR

In a **Local Earth view**, open **Layers → Hillshade** and choose
**WA LiDAR: bare earth** or **WA LiDAR: top surface**, then enable Hillshade.
**Settings → Paths → Stream detailed terrain** must be enabled. Pan or use the
terrain city search to visit Washington; Seattle is a useful starting point.

- **Bare earth** shows ground relief with vegetation and buildings removed.
- **Top surface** includes the surface of trees and buildings.

The layer controls credit the Washington Geological Survey (Washington
Department of Natural Resources) and link to the
[Washington Lidar Portal](https://lidarportal.dnr.wa.gov/) for individual surveys,
capture metadata, DEM/DTM/DSM downloads and point clouds. Coverage and capture
dates vary. The source choice lasts for the current session; USGS remains the
startup default and Hillshade starts off.

## Integration

Images come from DNR's public
[Lidar Hillshade service](https://gis.dnr.wa.gov/site3/rest/services/Geology/Lidar_Hillshade/MapServer),
using its composite bare-earth layer (1) or top-surface layer (0). These shaded
images retain their grayscale detail against the dark map and are projected at
sea level beneath the contour overlay. They do not
replace elevation samples, generate 3D buildings, or load LAS/LAZ point clouds.
Existing contour and marker heights continue to use their elevation sources.

Only the current view is requested, on one background worker. DNR requests
time out after 30 seconds and accept at most 8 MiB; decoded images are limited
to 2048 pixels per edge (the viewer requests 1024). Nearby views reuse the
texture, failures back off, and transparent coverage gaps remain transparent.
Images stay in memory; there is no state-wide download or new terrain cache.

## Verification

On 2026-10-13, the public service advertised both composite layers and the live
test passed for both Seattle products (distinct, visible relief). All eleven
offline hillshade tests passed. They cover coordinates, product selection,
coverage screens, bounds/size limits, alpha handling, stale worker completion
and retry behavior. The opt-in live smoke test checks both 256×256 Seattle
products for visible relief and distinct pixels:

```sh
cargo test -p one-thousand-electric-eye-desktop hillshade
cargo test -p one-thousand-electric-eye-desktop live_washington_hillshade -- --ignored --nocapture
```
