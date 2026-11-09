# 1kEE native VR

## Launch

Start SteamVR, wake the Index and its controllers, and check base station tracking. Run the viewer from the repository root.

```sh
QT_QPA_PLATFORM=xcb cargo run --release -p one-thousand-electric-eye-vr
```

The program uses the active OpenXR runtime, the GPU driving the headset, and the desktop settings beside the executable. The Qt environment setting works around an xcb-only SteamVR monitor on this Wayland workstation.

## Wrist controls

Lift the left controller. The panel follows the wrist and faces your eyes. **Left A** hides or shows it. Point with the right controller and click the trigger. The right stick scrolls while the pointer is over the panel. Looking around, leaning and turning your head continue at the headset’s frame cadence.

| Menu | Actions |
| --- | --- |
| Navigate | Select Globe or Local terrain, turn or zoom the view, adjust relief and Atlas lighting, recenter the table, open desktop settings, control replay or exit VR. |
| Layers | Use the native layer drawer, including contours, coastlines, terrain, infrastructure, imported data, and live markers. |
| Satellite | Toggle daily VIIRS true color or three-hourly EUMETSAT infrared, change opacity, inspect the frame timestamp and provider status, or import a georeferenced GeoTIFF. |
| Events | Select a geolocated event or enable Follow Events and adjust its dwell time. |
| Brief | Read the selected event’s title, severity, location, summary and available Factal detail. Pin or unpin it above the map. |
| Webcams | Play the nearest eligible snapshot/MJPEG feed for an event or choose one from the camera registry. Pin the feed with the brief or stop playback. |

Away from the wrist, the left stick turns the globe or pans Local terrain. The right stick zooms. Point at an event or camera marker and squeeze the right trigger to select it. Selection opens the corresponding wrist page and room panel. Follow Events uses the desktop scheduler, including regional recaps. Manual navigation releases follow control. Source availability follows the desktop’s provider settings, credentials and cache coverage.

## Terrain and imagery

Atlas opens by default. The globe uses geographically registered images from the native map renderer. Local shows a north-up terrain table with binocular relief derived from the active Atlas elevation raster. Both eyes share one published geographic view. Zooming and panning drive the desktop’s tile selection and terrain refinement, including GEBCO, SRTM and available finer surveys. Local retains a flat map where measured heights are unavailable.

The native operator has its own worker and Vulkan queue. It drives the existing sources, satellite refresh, layer rendering, event scheduler and webcam decoder. The headset draws from completed images and uploads new ones in bounded strips. A new map, its pick targets, and its height grid become visible together. Labels, terrain and imagery refresh at the operator’s cadence. Head pose and controller pointers update on each XR frame.

VR reads the asset root, satellite cache and event cache from the desktop configuration. An explicit root applies to the VR session. The release binary also accepts an event database or an initial imported layer.

```sh
target/release/one-thousand-electric-eye-vr --asset-root /run/media/m/Minkowski
target/release/one-thousand-electric-eye-vr --events target/release/.1kee_events.sqlite
target/release/one-thousand-electric-eye-vr --import /path/to/georeferenced-pass.tif
```

VR starts its event queue with the latest cached day and polls the configured live providers. GeoTIFF, GeoJSON, KML and KMZ imports use the desktop import path. Use **Browse files on wrist** in Layers or Satellite to navigate folders and import with the controller. The desktop file-picker button also remains available on the workstation. Source credentials can be configured in the desktop settings before launch. The standalone VR app does not require the desktop app to be running.

## Check frame pacing

Use the normal viewer to test the full operator while exploring an uncached region. A bounded session logs the XR frame period, CPU p95, stereo GPU p95 where timestamps are available, and gaps between predicted display times. SteamVR Advanced Frame Timing reports compositor timing and reprojection.

```sh
QT_QPA_PLATFORM=xcb target/release/one-thousand-electric-eye-vr --frames 600
```

The original isolated globe remains available with `--demo`. The diagnostic options `--no-terrain`, `--stream-after N` and `--tile DB Z Y X` also select that path. Its built-in scene has a Mount Rainier reference marker and a single 400 m SRTM contour tile. `--display-test` uses a head-relative reference space for checking display output. Run `--help` for all launch options.

```sh
ONEKEE_GPU='RTX 4090' cargo test --release -p one-thousand-electric-eye-vr -- --ignored --nocapture --test-threads=1
```

The GPU tests exercise wrist clicks, event picking, Follow Events, GeoTIFF import, geographic tile changes, terrain displacement and per-eye registration while images upload. Set `ONEKEE_VR_ASSET_ROOT` to include an additional check against measured Mount Rainier terrain on a data drive. Set `ONEKEE_VR_CAPTURE_DIR` to an existing directory to save the native map and panels, or `ONEKEE_VR_STEREO_PNG` to save the diagnostic stereo pair.

On November 9, 2026, SteamVR opened the Index on the RTX 4090 with two 2468 × 2740 eye targets. The full operator hardware run submitted one stereo frame before SteamVR stopped requesting visible frames. A separate one-frame run exited cleanly at an 8.33 ms display period. That cold frame took 126.77 ms of application wall time including image acquisition and submission. The offscreen local-terrain and wrist test at the same resolution measured 0.24 ms p95 for the two GPU passes and 0.70 ms p95 from starting the pair through GPU completion during map uploads. The drive-backed check loaded and colored the measured Mount Rainier Local view in 1.80 seconds. The stereo timings use synthetic poses and exclude physical tracking, source rendering on the independent worker, compositor work and scanout.

The fallback Earth texture comes from Natural Earth shaded relief. The diagnostic tile comes from SRTM GL1 N46W122, simplified at 0.002° and encoded as CTF1. [Natural Earth](https://www.naturalearthdata.com/downloads/10m-raster-data/) · [NASA SRTM](https://www.earthdata.nasa.gov/data/instruments/srtm)
