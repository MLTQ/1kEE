# roundtrip_parity.rs

## Purpose

Locks down the `.1kc` serialization contract used by the cache builder and desktop
renderer. The test compares feature order and floating-point bit patterns rather
than approximate values, so output-neutral performance changes cannot silently
alter cached geometry.

## Components

### `cell_round_trip_preserves_feature_order_and_float_bits`

- **Does**: Writes two ordered feature chunks and verifies that the reader returns
  the same metadata, geometry, elevation presence, and `f32` bit patterns.
- **Interacts with**: `write_cell` in `../src/write.rs` and
  `read_single_chunk` in `../src/read.rs`.
- **Rationale**: Visual parity depends on preserving source vertex ordering and
  exact coordinates; approximate float assertions would not detect a format
  regression.

## Contracts

| Dependent | Expects | Breaking changes |
|---|---|---|
| Cache-builder optimizations | Performance changes retain decoded feature semantics | Changing output order or coordinate/elevation representation |
| Desktop cache loaders | Existing `.1kc` data remains readable without coordinate drift | Changing tag/chunk decoding behavior |

## Notes

- This is a format-level parity gate, not a GPU screenshot test.
- It intentionally includes signed zero and a NaN payload to detect lossy float
  normalization.
