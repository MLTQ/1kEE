# event_overlay_preview.rs

## Purpose
Render the actual selected map card, five-row chyron and Brief through egui/wgpu
to inspect native text, clipping, colors, scrolling and reserved bottom space.

## Components
- `five_line_chyron_gpu_preview` is opt-in/ignored and uses a private 1280×720
  texture. It exports start and scrolling frames to `target/event-chyron-qa`, or
  `ONEKEE_EVENT_OVERLAY_PREVIEW` when set.
- The example requested by the user is the selected fixture; other rows are
  explicitly sample reports. The grid stands in for terrain so QA is independent
  of live sources, cached tiles or an app restart.

## Contracts
All records are test-only and never enter runtime state. No network, source
polling, settings writes, native app control or public streaming occurs.
