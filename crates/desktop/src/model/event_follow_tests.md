# event_follow_tests.rs

Exercises real controller/model behavior with deterministic event fixtures and
absolute frame times: poll deduplication, burst selection, no synthetic/USGS
arrivals, near/far ten-second flights in both projections, date-line wrapping,
bounded polar motion, position/velocity continuity on retarget and orbit entry,
selection/brief retention through feed replacement, and cancellation.

A headless egui regression paints the cinematic brief after its record leaves
the live page and checks that the title remains to the right of map center.
