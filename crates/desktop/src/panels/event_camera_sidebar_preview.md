# event_camera_sidebar_preview.rs

## Purpose
Opt-in native egui/wgpu rendering of the real camera sidebar and Brief in normal
and OBS views, using inert test-only camera textures and no source polling.

## Components
- `event_camera_sidebar_gpu_preview` exports two 1280×720 PNGs to
  `target/event-camera-qa` or `ONEKEE_EVENT_CAMERA_PREVIEW`.
- Uses the same render path and primed players as layout regression tests.
- Includes a collapsed unavailable feed among three live previews to check
  compact spacing and retained live image area.
- Requires a GPU and is ignored by the ordinary desktop test run.

## Contracts
No network requests, settings writes, app restart or runtime synthetic records.
