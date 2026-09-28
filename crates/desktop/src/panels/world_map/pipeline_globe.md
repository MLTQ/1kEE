# pipeline_globe.rs

Loads the public pipeline overview and prepares unit-sphere line instances on a
background worker. Reuses the established globe GPU line pass with its own slot.
Camera orbit/zoom updates uniforms only; the batch never depends on projection.

Keys include source root, filters and palette. Reload generations reject stale
jobs, buffers use monotonic versions, and large retired snapshots are dropped
outside the shared lock. One worker is active at a time. No arbitrary line budget
truncates routes. Source/status controls and provenance are shared with the local
layer; per-route hover details are currently available in local view.

The ignored real-data test checks every eligible overview segment reaches the
GPU batch and measures one-time preparation without touching the source archive.
