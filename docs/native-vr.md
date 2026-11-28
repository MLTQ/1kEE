# 1kEE native VR

## Launch

Start SteamVR, wake the Index and its controllers, and check base station tracking. Run the viewer from the repository root.

```sh
QT_QPA_PLATFORM=xcb cargo run --release -p one-thousand-electric-eye-vr
```

The program uses the active OpenXR runtime, the GPU driving the headset, and the desktop settings beside the executable. The Qt environment setting works around an xcb-only SteamVR monitor on this Wayland workstation.

## Forearm Wrist HUD (Half-Life: Alyx style)

Raise your left forearm as if checking a watch to wake the compact diegetic HUD (`17.5 × 13.2 cm`) mounted physically to your wrist. Lowering your arm fades it out cleanly; **Left A** (Index) / **Left X** (Touch) or the HUD's **PIN** button locks it open. The wrist HUD renders directly onto the OpenXR `wgpu::Device` every stereo frame (zero CPU readback), with trigger-stabilized laser targeting and haptic ticks on hover, press, spire pick, and grab.

| Tab | Actions |
| --- | --- |
| **MAP** | Live continuous scale readout (`24,000 km` → `20 m`), 1-tap scale presets (`ORBIT` / `REGION` / `CITY` / `STREET`), continuous logarithmic scale slider, `PERSP` / `ORTHO` toggle, theme cycler, `FOLLOW`, `CENTER`, `HOLOPAD`, `REPLAY`, and `EXIT`. |
| **LAYERS** | Featured **LIDAR CLOUD (FUSED)** layer toggle (default `1` px point size + live size slider & point count), 2-column tactical toggles (Contours, Coastlines, Bathymetry, Roads, Buildings, Water, Forest, Weather IR, Events, Webcams, Ships, Flights, Seismic), and 3D relief slider. |
| **EVENTS** | Live geolocated event list with severity badges and `FOLLOW EVENTS` toggle. |
| **BRIEF** | Full selected event title, severity, location, summary, and floating room holopad pin. |
| **CAMS** | Nearby snapshot/MJPEG webcam switcher, auto-nearest toggle, and holopad playback controls. |

## Continuous Grab & Stick Navigation

- **1-Hand Grip Grab**: Squeeze either controller's grip to physically grab and spin the 3D orbital globe or pan the 3D stereo relief table 1:1.
- **2-Hand Grip Pinch/Spread**: Squeeze both grips and spread hands apart to zoom continuously from `24,000 km` planet scale smoothly into the holographic relief table all the way down to `20 m` street/LiDAR scale (pinch together to pull back out to orbit).
- **Thumbsticks (Fallback)**: Left stick orbits/pans proportionally to the current ground span; right stick zooms continuously (or scrolls when pointing at the wrist HUD).
- **Spire Picking**: Point at any glowing event or webcam spire and pull the right trigger to focus it and open its brief/feed on the floating room holopad.

## Terrain and imagery

Atlas opens by default. The native map renderer captures the hemisphere facing the tracked headset. Walking around the globe changes the requested map coverage while the globe stays anchored in the room. Event and webcam spires are stereo geometry. They rise perpendicular to the globe or the measured Local surface, and terrain occludes them in each eye. Local shows a north-up terrain table with binocular relief derived from the active Atlas elevation raster. Both eyes share one published geographic view. Zooming and panning drive the desktop’s tile selection and terrain refinement, including GEBCO, SRTM and available finer surveys. Local retains a flat map where measured heights are unavailable.

The native operator has its own worker and Vulkan queue. It drives the existing sources, satellite refresh, layer rendering, event scheduler and webcam decoder. The headset draws from completed images and uploads new ones in bounded strips. A new map, its pick targets, and its height grid become visible together. Labels, terrain and imagery refresh at the operator’s cadence. Head pose and controller pointers update on each XR frame.

VR reads the asset root, satellite cache and event cache from the desktop configuration. An explicit root applies to the VR session. The release binary also accepts an event database or an initial imported layer.

```sh
target/release/one-thousand-electric-eye-vr --asset-root /run/media/m/Minkowski
target/release/one-thousand-electric-eye-vr --events target/release/.1kee_events.sqlite
target/release/one-thousand-electric-eye-vr --import /path/to/georeferenced-pass.tif
```

VR starts its event queue with the latest cached day and polls the configured live providers. GeoTIFF, GeoJSON, KML and KMZ imports use the desktop import path. Open **Satellite → Browse files on wrist** to navigate folders and import with the controller. **Layers → Import layers…** opens the same page. Source credentials can be configured in the desktop settings before launch. The standalone VR app does not require the desktop app to be running.

## Check frame pacing

Use the normal viewer to test the full operator while exploring an uncached region. A bounded session logs the XR frame period, CPU p95, stereo GPU p95 where timestamps are available, and gaps between predicted display times. SteamVR Advanced Frame Timing reports compositor timing and reprojection.

```sh
QT_QPA_PLATFORM=xcb target/release/one-thousand-electric-eye-vr --frames 600
```

The original isolated globe remains available with `--demo`. The diagnostic options `--no-terrain`, `--stream-after N` and `--tile DB Z Y X` also select that path. Its built-in scene has a Mount Rainier reference marker and a single 400 m SRTM contour tile. `--display-test` uses a head-relative reference space for checking display output. Run `--help` for all launch options.

```sh
ONEKEE_GPU='RTX 4090' cargo test --release -p one-thousand-electric-eye-vr -- --ignored --nocapture --test-threads=1
```

The GPU tests check both eyes from opposite sides of a globe with known elevation bands, visible and hidden spires, the controller beam, and wrist clicks interleaved with map and room redraws. They also exercise Follow Events, GeoTIFF import, geographic tile changes and terrain displacement during uploads. Set `ONEKEE_VR_ASSET_ROOT` to include an additional check against measured Mount Rainier terrain on a data drive. Set `ONEKEE_VR_CAPTURE_DIR` to an existing directory to save the native map and panels, or `ONEKEE_VR_STEREO_PNG` to save the diagnostic stereo pair.

On November 9, 2026, the repaired viewer completed 240 tracked stereo frames on the Index in a focused SteamVR session at 120 Hz, with 2468 × 2740 pixels per eye. The first 120 frames recorded seven predicted display gaps and 17.33 ms application CPU p95 including image acquisition. The next 120 recorded zero gaps and 4.03 ms CPU p95. Stereo GPU p95 was 0.16 ms in both intervals. The separate offscreen tests use synthetic head and hand poses to check geographic coverage, marker occlusion and controller targeting. Physical trigger feel and compositor-to-display latency require an operator in the headset.

The fallback Earth texture comes from Natural Earth shaded relief. The diagnostic tile comes from SRTM GL1 N46W122, simplified at 0.002° and encoded as CTF1. [Natural Earth](https://www.naturalearthdata.com/downloads/10m-raster-data/) · [NASA SRTM](https://www.earthdata.nasa.gov/data/instruments/srtm)
