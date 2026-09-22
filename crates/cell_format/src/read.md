# read.rs

## Purpose
Reads v1 binary vector cells. `read_single_chunk` scans envelopes and decodes only
the requested layer, avoiding allocations and decoding for unrelated chunks.

## Contracts
- Existing cell schema/coordinates/elevations remain unchanged.
- Requested malformed/truncated chunks return None, never a successful empty
  layer. Unknown/unrequested payloads are skipped by length without decoding.
- Counts are bounded by remaining payload bytes before reserving vectors.
- `read_chunks` retains its legacy all-chunk behavior for existing callers.
