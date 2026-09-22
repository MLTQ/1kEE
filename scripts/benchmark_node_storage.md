# benchmark_node_storage.py

## Purpose
Measures the opt-in compact node backend against flat storage using one release
binary and a bounded read-only PBF extract. Never targets existing cache folders.

## Contracts
- Alternates execution order each round; both modes use fresh system-temporary
  working/output directories, automatically removed after each run.
- Elapsed time includes all indexing, feature processing and output; excludes
  verification and cleanup. Scratch bytes are final logical file sizes, excluding
  source and generated cells, not a sampled peak-space metric. On macOS the
  system `time -l` command also measures peak resident bytes (null elsewhere).
- Imports exact feature signatures from `benchmark_planet_builder.py`, ignoring
  only unordered feature serialization order. Any parity difference fails.
- JSON lines include source, time, scratch bytes and verified geometry counts.
- Results depend on source locality, cache warmth and output volume; small
  extracts do not predict full-planet random-read throughput.

## Usage
`python3 scripts/benchmark_node_storage.py --builder
target/release/one-thousand-electric-eye-cache-builder --source /path/to/extract.osm.pbf`
(one command line).
