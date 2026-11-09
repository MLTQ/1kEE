# atlas_panel.rs

Child module of the cache-builder app. Provides source/output selection, global
or bounding-box coverage, tier selection and a background Atlas job. Initial
paths derive from the existing OSM form; Atlas output lives beside OSM under
Derived/terrain, rather than inside the OSM cache. Land detail is opt-in.

The dashboard consumes Atlas progress events with completed, rebuilt, reused,
remaining, unavailable-SRTM and byte counters. It does not scan OSM inventory
while the Atlas tab is selected. File selection/validation errors are shown
without starting a job. Completed tiles are durable; the form and its current
run counters are session-local.
