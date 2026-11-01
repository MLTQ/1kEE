# event_follow_tests.rs

Exercises the event tour and camera controller with deterministic event fixtures
and absolute times. It covers numeric severity and category fallback, recency ties,
the full unique pool, invalid/non-Factal exclusion, ranking revisions, refreshes
that preserve tour progress and complete dwell, configurable post-arrival dwell, single/empty payloads,
and automatic brief reopening without a camera snap.

Motion tests compare nearby/regional/global duration (never above ten seconds),
arrival positions and velocities, date-line wrapping, bounded polar motion,
queued retarget and orbit-entry continuity (including zoom phases of short flights) in both Globe and Local views, feed selection
retention and manual/replay cancellation. A headless egui regression paints the
followed-event brief after its record leaves the live page and checks placement to the
right of map center.

Card-opacity regressions cover hidden initial/wide travel, arrival at the actual
Globe/Local landing zoom, eased fade-in and departure fade-out, cancellation,
unchanged and newer-headline payload refreshes, and a complete arrival fade/dwell
before departing for a new story. Long-run weighted coverage and repeat spacing
are tested independently in `event_tour_tests.rs`.

Configurable timing checks 5/30/90/300-second dwell after arrival in both views.
Changing dwell mid-stop preserves camera pose and arrival time, postpones the
deadline when lengthened, and advances next tick if a shorter dwell has elapsed.
Stop/enable retains the setting; out-of-range values clamp to shared bounds.
