# diagnostics_tests.rs

Fixtures verify that corrupt databases and invalid schemas remain errors,
source bytes are preserved, missing caches are not created, valid zero-line
tiles remain successful, recovered tiles can coexist with an unreadable original,
invalidation markers do not imply readiness, nested current replacements take
priority, and recovery probes remain bounded.

The opt-in `cache_diagnostics_live_minkowski_recognizes_working_recovery` test
reads original manifest metadata and a bounded recovery sample on the attached
drive. Set `ONEKEE_TERRAIN_REPAIR_ROOT` to the app's configured destination so the
test executable uses the same recovery routing. It neither rebuilds terrain nor
certifies every cached tile.
