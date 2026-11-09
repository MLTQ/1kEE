# 1kEE native VR

The Linux VR executable uses OpenXR, Vulkan, wgpu and egui. SteamVR chooses the graphics adapter that drives the Index. The desktop and VR shaders share the globe’s terrain shading and graticules. The VR pass intersects that sphere from each eye using the runtime’s predicted pose and asymmetric field of view.

## Run on a Valve Index

1. Start SteamVR with the Index connected. Check that the headset sees its base stations and that SteamVR reports tracking.
2. From the repository root, run `cargo run --release -p one-thousand-electric-eye-vr`. The program uses the active OpenXR runtime and needs the Vulkan and OpenXR loaders.
3. Look ahead when the first tracked frame arrives. The globe sits about a metre in front of you. Lean sideways to see the marker and contours stay attached to the surface.
4. Use the left thumbstick to turn the globe. Push the right stick up to zoom into the Rainier contour tile and down to zoom out. Aim the right controller at the orange marker and click the trigger to read its egui brief. Click elsewhere on the globe to close it. Quit through SteamVR.

On this Wayland workstation, SteamVR’s Qt monitor reported that only its xcb plugin was available. Prefix the launch command with `QT_QPA_PLATFORM=xcb` if the monitor gives that error.

The default marker identifies the summit of Mount Rainier. To open a real event instead, point the viewer at the desktop app’s event database. This reads the most recent geolocated event and summary at startup. The desktop stores that file beside its executable.

```sh
target/release/one-thousand-electric-eye-vr --events target/release/.1kee_events.sqlite
```

## Measure frame pacing

Use the same headset refresh rate, resolution and head movements for both runs.

```sh
target/release/one-thousand-electric-eye-vr --frames 600 --no-terrain
target/release/one-thousand-electric-eye-vr --frames 600 --stream-after 240
```

Every 120 rendered frames the viewer reports the runtime’s frame period, p95 application CPU wall time from frame wait return through submission/end, p95 GPU time across the two eye passes when timestamp queries are available, and gaps between predicted display times. SteamVR’s Advanced Frame Timing view supplies compositor timing and reprojection. Motion-to-photon latency requires a separate measurement. A finite run reports when SteamVR stops supplying visible, tracked frames. The optional `--display-test` places the scene relative to the headset for a stereo display diagnostic.

The tile reader runs on a worker thread. GPU storage is preallocated, and each frame submits at most 48 KiB of new line vertices. Both eyes begin drawing a tile on the same frame once all its vertices have been submitted. Time from the tile request to final submission appears on its own log line. Use `--tile PATH Z Y X` to read a specific native SQLite contour tile in place of the included sample. The prototype displays one tile with a maximum of 131,072 segments. The initial view faces the marker. Rotate to the location of a custom tile.

To reproduce the offscreen tests at the eye dimensions SteamVR reported on this workstation:

```sh
ONEKEE_GPU='RTX 4090' cargo test --release -p one-thousand-electric-eye-vr -- --ignored --nocapture --test-threads=1
```

The pixel test reads back both eyes after a sideways translation, checks marker registration and limb occlusion, and checks that the selected brief draws text. Set `ONEKEE_VR_STEREO_PNG` to a file path to save the pair. The timing test draws two 2468 × 2740 views for 240 frames, starting tile uploads halfway through. It uses synthetic poses and waits for GPU completion in the test harness.

## Included data

- `earth.png` is a 1024 × 512 grayscale derivative of Natural Earth’s Gray Earth shaded relief with ocean bottom, resampled from the desktop terrain source. [Natural Earth raster data](https://www.naturalearthdata.com/downloads/10m-raster-data/)
- `rainier.ctf` contains 400 m contours from the SRTM GL1 tile N46W122. The lines were generated with GDAL, simplified at 0.002°, and encoded in the repository’s CTF1 format. [NASA Shuttle Radar Topography Mission](https://www.earthdata.nasa.gov/data/instruments/srtm)

## Workstation check, November 9, 2026

SteamVR selected the RTX 4090 and allocated two 2468 × 2740 eye targets for the Index. The native viewer submitted a stereo pair using a cached desktop event. A separate one-frame run reported an 8.33 ms display period and exited cleanly through SteamVR. SteamVR then stopped requesting visible frames, and the lighthouse log reported no base stations seen. Sustained headset timing and physical controller selection remain to be checked.

With the same eye sizes on the 4090, the offscreen stereo GPU p95 was 0.07 ms in both the globe-only and contour-upload/resident windows. The full-size test read back the marker from both rendered targets. This measures a single tabletop globe, one brief, and one small tile. It excludes tracking, compositor work, scanout and large regional datasets.
