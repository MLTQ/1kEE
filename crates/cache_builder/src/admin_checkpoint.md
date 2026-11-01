# admin_checkpoint.rs

Durable planet assembly checkpoint, separate from the completed scan databases.
A source/version identity owns saved relation blobs and on-demand node coordinates;
changing it resets only these derived tables. Each region is a committed complete
unit, including honest incomplete topology and empty results. Coordinate NULLs
mean known missing nodes and never bridge geometry gaps.

All missing references of a relation resolve together, then persist for neighboring
regions, finer levels and restarts. Coordinates round-trip f32 through SQLite REAL.

Publication streams saved 1kc relation records into one compatible ADMN chunk,
using at most one region of memory. Counts and chunk lengths are checked, output
is synced then atomically renamed, and the previous file survives an error.
The 22-byte header layout must match cell-format v1; regression tests decode output
with the public reader. Saved geometry survives interrupted publication and can be
republished to a new output directory without redoing any coordinate lookup.

Tests verify that both found and missing coordinates survive reopening, preserve
reference order/duplicates, and reset with saved geometry when the source changes.
