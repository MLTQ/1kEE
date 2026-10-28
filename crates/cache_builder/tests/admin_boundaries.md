# admin_boundaries.rs

Exercise the public CLI on a committed synthetic PBF. Flat and compact builds
must emit byte-identical administrative output for every level 2–11. Verify
reversed member stitching, inner holes, disconnected islands, missing nodes,
missing ways and admin backfill after an already-completed compact way build.
All output and scratch state belong to disposable temporary directories.

A focused import over existing global output must preserve complete country rings and holes.
