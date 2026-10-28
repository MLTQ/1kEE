# build.rs

Streams normalized JSONL route parts, validates coordinates and attribute sizes,
clips connected geometry into spatial tiles, and builds a simplified overview.
Retains raw source manifest, source/owner/operator/status/accuracy per fragment.

Publication uses a unique staging database, closes SQLite, then hard-links a new
destination without replacing existing data. Errors remove only owned staging
files. No source downloads or PBF scans occur here. Progress counts actual parts
and vertices processed. Full tile geometry retains all source vertices plus
boundary intersections; only overview geometry is simplified.

Optional capacity metadata survives every clipped fragment, overview and packed archive. Nonfinite/negative values or empty unit/release fields fail validation before publication. Old JSONL without capacity still builds.
