# follow_recap_tests.rs

Brief rendering supplies an inert radio controller; these fixtures do not query radio or play audio.

Deterministic timing/coordinate tests for automatic regional recaps. Fixtures
are test-only and API keys are cleared; no network requests or settings writes.
Tests replace the short-lived SQLite receiver with known data.

- All eight regions, representative cities/islands and date-line positions;
  invalid coordinates and a global coordinate grid.
- Full rotation and wraparound with five live minutes, framing, 60-second
  playback, visible final clock, restored Globe/Local mode and retained dwell.
- Rolling-day endpoints, chronological sorting and exclusion of other regions.
- Loading holds the clock; skipped frames expire old flares; missed time does
  not cause repeated recap catch-up.
- Incoming live refreshes preserve playback; manual navigation, body switch,
  Follow toggle and manual Replay cancel automatic resumption.
- Manual replay settings remain independent.

Real egui panel output also verifies that both normal and OBS recaps reserve
no camera-sidebar width and hide the Brief while preserving its open flag;
the sidebar paints again after live resumption.

Middle East fixtures cover Egypt, Turkey, the Levant, Iran and Gulf/Yemen cities,
plus neighboring regions. Loading every zone from one shared history verifies
that each report appears exactly once. The rotation test checks each region's
chosen camera zoom after framing.

The Caucasus fixtures preserve the pre-existing Asia assignment; the new Middle
East zone does not capture those neighboring capitals.
