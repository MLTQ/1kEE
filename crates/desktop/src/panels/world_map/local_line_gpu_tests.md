# local_line_gpu_tests.rs

## Purpose
Opt-in hardware regression exercising the real local line shader through egui's
GPU callback preparation and painting, using a private 64×64 offscreen texture.

## Coverage
- Roads and contours occupy disjoint cache keys and uniform slots.
- Four joined road chunks exceed the three-upload cap, then complete next frame
  even without a background contour pass. No road segment is permanently omitted.
- Readback confirms continuous pixels at all road joins and both layer widths.
- Test creates no window and does not alter user caches, settings or running app.
- Run with `cargo test -p one-thousand-electric-eye-desktop
  roads_and_contours_render_together -- --ignored --nocapture` (one line).

The same hardware test includes residency cleanup before draw callbacks. It
checks that later road callbacks survive cleanup, absent layers release their
buffers even below budget, a frame without line draws reaches zero bytes, and
returning roads reupload completely across the normal upload limit.
