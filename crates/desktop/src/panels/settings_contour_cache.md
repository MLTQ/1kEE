# settings_contour_cache.rs

Files & Cache shows separate SRTM and detailed-terrain diagnostics. Original
database errors remain errors instead of becoming a zero count or a guessed
wrong-path warning. A valid empty cache or zero-line tile is not evidence that
contour generation failed.

Recovered tile availability is independent of original database health. The
panel can show working recovery alongside an unreadable original. An expandable
section gives the original/recovery locations, original per-level totals and a
readable recovery-manifest example. Original totals exclude recovered tiles;
availability checks do not certify all cached geometry or coverage.

All contour-cache discovery and SQLite reads use a single background
`layer_snapshot` worker. Compatible previous results stay visible during a
five-second refresh. A new asset root rejects previous-root results, and refresh
generation cannot change while a slow scan is pending. Paint only polls metadata.
The panel does not relocate, regenerate, rewrite or delete any terrain data.

Tests exercise the rendered error/recovery text, legitimate zero-line tiles and
slow-scan refresh identity. The backend has read-only fixture and opt-in live
cache tests.
