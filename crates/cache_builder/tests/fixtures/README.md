# Planet scan fixtures

Synthetic four-node, two-way input for ordered parallel scan and restart tests.
`planet-tiny.osm` is the editable source. Both PBF encodings are committed so
normal tests require no osmium, network access, or external datasets.

Regenerate with `osmium cat planet-tiny.osm -o planet-tiny-dense.osm.pbf
-f pbf,pbf_dense_nodes=true` and repeat with `plain` / `false` (one command line).

`admin-tiny.osm` / `admin-tiny.osm.pbf` exercise levels 2–11, reversed outer
members, an inner hole, an island, a missing node and an absent member way.
Coordinates and names are entirely synthetic. Regenerate with:
`osmium cat admin-tiny.osm -o admin-tiny.osm.pbf -f pbf,pbf_dense_nodes=true`.
