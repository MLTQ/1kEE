# infra_layer_tests.rs

Exercises the source preparation and shape factory used by Local infrastructure.
Airport property rings must remain unfilled, runway lines must stay independent
and prominent, and concave terminal/government triangles must preserve courtyard
area. An existing-format binary fixture proves closed runway footprints work
with the historical false polygon flag. Missing projections split paths, open
area flags do not invent closing edges, and date-line strips keep short edges.

Opt-in live tests read the existing Nashville aeroway cell on Minkowski. The GPU
test uses the real Local projector and shared offscreen egui/wgpu harness, saving
`/tmp/1kee-airports-preview.png`. It does not regenerate cells, alter app settings
or claim extra pavement detail absent from the source cache.
