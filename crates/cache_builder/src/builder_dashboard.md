# Cache builder dashboard

The center panel shows a live build summary, source scan progress, durable resume
position, elapsed time, scan rate/ETA and tile throughput. The per-layer inventory
counts current `.1kc` files and bytes rather than just legacy GeoJSON files.
Selected layers without binary output appear explicitly as missing. Selecting a
layer shows a world-coordinate cell-presence map. Presence is not completeness:
planet scans may have written only some ways into any of these cells.

One background worker scans the selected output directory's layer folders every
15 seconds (or on Refresh). It never reads entire feature payloads. Scans do not
overlap; changing output, working directory or backend discards stale results.
Errors are surfaced, not represented as successful empty coverage. Net file/byte
growth compares consecutive inventories of the same configuration. Legacy files
are counted separately, preventing duplicate binary/GeoJSON totals.

The saved-state panel reads the selected backend's existing checkpoint and the
new build-status report. A zero legacy feature offset is ambiguous (unstarted or
finished), so the UI never treats it as proof of completion. Compact node state
is not inferred from legacy checkpoint fields. Existing caches without telemetry
remain usable and report completeness as unknown.

The configuration sidebar and central dashboard scroll independently. Logs are
bounded by the app and can be copied. A pure GPU offscreen preview test writes
`/tmp/onekee-builder-dashboard.png`; `ONEKEE_PREVIEW_WIDTH` supports narrower
previews (a multiple of 64 pixels). The ignored `inspect_operator_cache` test
accepts `ONEKEE_CACHE_INSPECT_ROOT` for a read-only real-data inventory check.

Hidden macOS AppleDouble metadata (`._*`) is excluded at both folder and file
levels. The real migrated cache contains these sidecars; counting their `.1kc`
extensions would nearly double file totals and report fake layer errors.
