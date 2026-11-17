# diagnostics.rs

Read-only Settings diagnostics for `srtm_native_v1.sqlite` and the detailed
`srtm_focus_cache.sqlite`. Paths share the renderer's cache-directory resolution,
including its temporary-directory fallback, without creating that directory.

Original manifests are grouped by zoom level in one query. Row, schema, open and
filesystem errors propagate into an unreadable state; they never become zero
tiles or lines. Reads use the same query-only/native-provenance/immutable fallback
helper as rendering. No geometry scan, database repair or cache mutation occurs.

Recovery roots and per-tile resolution come directly from `recovery`. At most 32
directory entries are inspected per cache; the first readable manifest row at
its currently resolved location proves some recovered tiles are available.
Invalidation-only folders and superseded replacements do not prove completion.
A recovery directory with no verified sample is reported as present, without
claiming that it is empty. Recovery totals are deliberately not inferred from
directory counts, which include unfinished invalidation markers.

This bounded check matters on Minkowski: there are thousands of independent
replacement databases, while original manifest queries still report malformed
SQLite pages. A full directory/database scan is unsuitable for a Settings frame.
Verified live examples include native level 4 tile `126,143` (3,633 manifest
lines) and detailed level 8 tile `1240,-2811` (1,069 lines). These samples do not
certify the entire cache or underlying device.
