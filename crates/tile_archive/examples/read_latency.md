# read_latency.rs

## Purpose
Reproducible vector read/decode benchmark using one real cell and a new disposable
archive on a chosen volume. Runs 49 small view windows plus one whole-cell comparison in alternating order,
checking every encoded feature byte for parity after each measurement.

## Usage
`cargo run --release -p tile-archive --example read_latency -- CELL.1kc NEW.1ka`

## Contracts
- Original cell is read-only. Destination must not exist and is retained for
  inspection; caller chooses/removes only its own benchmark archive.
- Both paths retain full detail and apply identical AABB filtering/deduplication.
- Timings include opening, reading, decoding and filtering, plus archive source
  stamp validation. Exclude parity verification, rendering and terrain sampling.
- Cache warmth is uncontrolled; conversion warms data. No cold-disk or complete
  pan-to-paint speed claim. Existing contour benchmark covers its separate path.
