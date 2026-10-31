# event_overlay_tests.rs

## Purpose
Verify the combined Follow Events presentation through the exact map overlay
entry point, independently of terrain/GPU/feed loading.

## Coverage
- Globe/Local and normal/Focus combinations each show five full-summary crawls
  and exactly one selected event card while the Brief is open.
- Closing the Brief hides all event cards; leaving follow removes the overlay.
- A deliberately long Brief at 720/800/1080 screen heights scrolls within its
  reserved height and leaves the five-line chyron plus local footer unobstructed.

## Contracts
Fixtures are test-only real-format Factal records. Rendering does not poll
sources, save settings or require the native app.

`event_overlay_preview.rs` supplies an opt-in GPU visual check of these actual
renderers with a private texture and test-only stories.
