# event_follow_tests.rs

Exercises the event tour and camera controller with deterministic event fixtures
and absolute times. It covers numeric severity and category fallback, recency ties,
six unique stops, invalid/non-Factal exclusion, ranking revisions, refreshes that
preserve tour progress, cycling, ten-second orbit dwell, single/empty payloads,
and automatic brief reopening without a camera snap.

Motion tests compare nearby/regional/global duration (never above ten seconds),
arrival positions and velocities, date-line wrapping, bounded polar motion,
retarget and orbit-entry continuity (including zoom phases of short flights) in both Globe and Local views, feed selection
retention and manual/replay cancellation. A headless egui regression paints the
cinematic brief after its record leaves the live page and checks placement to the
right of map center.
