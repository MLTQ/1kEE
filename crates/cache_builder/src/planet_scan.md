# planet_scan.rs

## Purpose
Decodes and encodes independent PBF node batches in parallel, moving decompression
and coordinate conversion off the single sequential planet-file reader/writer.

## Components
- `node_records`: emits the exact existing i64/f32/f32 little-endian records in
  source element order. Headers and ways produce no node bytes; errors propagate.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| Planet Pass 1 | Indexed parallel collection preserves blob order; one writer appends batches | Unordered writes or changing f32 conversion |
| Resume checkpoint | Node bytes are durable before the batch-end offset is saved | Advancing the checkpoint during parallel decoding |

## Notes
Pass 1 holds at most 64 compressed blobs plus their encoded node vectors and
active decoder buffers, independent of planet size. Existing external sorting
and checkpoint file formats remain unchanged.

## Validation
Committed synthetic ordinary/dense-node PBF fixtures verify bit-exact f32 records
and source order under parallel decoding. Malformed data blobs must return an
error rather than look like an empty successful batch.
