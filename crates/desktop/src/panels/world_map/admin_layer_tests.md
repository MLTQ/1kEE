# admin_layer_tests.rs

Verify canonical Derived/osm discovery, selected-level isolation, binary ring
grouping, hole-capable geometry, corrupt binary fallback, legacy outline-only
behavior and malformed coordinate rejection using disposable local caches.

Scratch/cache fixture helpers are shared with the optional GPU preview test.

An opt-in measurement runs the production globe and oblique Local camera requests
against a prewarmed real cache, reports submitted/candidate geometry and worker
time, and verifies that the national border is absent around central Berlin.
