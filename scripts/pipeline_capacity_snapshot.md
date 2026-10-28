# pipeline_capacity_snapshot.py

Creates the bundled, deterministic GEM capacity supplement from the exact raw
map snapshots configured in `fetch_pipelines.py`. No downloads, geometry changes
or unit conversion. Refuses to overwrite output. Rejects duplicate/missing IDs,
empty capacities and mixed units; normalization shares `capacity()` with ingest.

The result records each raw SHA256, source URL, original units, release and
CC BY 4.0 attribution. Consumers must match both the URL and raw checksum against
the archive manifest before joining by source/project ID. This supports archives
whose old importer discarded capacity without silently mixing releases.

```sh
python3 scripts/pipeline_capacity_snapshot.py --gas /path/gem-gas.geojson \
  --oil /path/gem-oil.geojson --out /path/new-capacity-snapshot.json
```
