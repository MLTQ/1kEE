# Planet scan fixtures

Synthetic four-node, two-way input for ordered parallel scan and restart tests.
`planet-tiny.osm` is the editable source. Both PBF encodings are committed so
normal tests require no osmium, network access, or external datasets.

Regenerate with `osmium cat planet-tiny.osm -o planet-tiny-dense.osm.pbf
-f pbf,pbf_dense_nodes=true` and repeat with `plain` / `false` (one command line).
