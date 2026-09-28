# tests.rs

Regression tests cover clipping continuity and re-entry, antimeridian crossings
in both directions, polar boundaries, source/status filtering, attribute/geometry
round trips, empty coverage, copying into a world archive containing OSM PIPE,
refusal to overwrite output, and staging cleanup after invalid input. All tests
use small disposable fixtures; no user data or network access is required.

The opt-in real-archive test is read-only. It times overview and four regional
loads (including Japan) and verifies every full-detail tile checksum, metadata,
and coordinate lies within its tile. Set ONEKEE_PIPELINE_ARCHIVE to run it.
