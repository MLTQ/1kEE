# planet_lookup.rs

## Purpose
Keeps feature classification/output identical for legacy flat-node and opt-in
compact PBF lookup. Each backend supplies an independent worker session.

## Contracts
- The same ordered optional coordinate results and error propagation reach
  `planet_all` regardless of backend.
- Compact source validation runs around each feature-processing batch and flush.
- Flat node files and their legacy resume state retain their existing behavior.
- `prepare_refs` primes compact sessions with bounded, grouped way dependencies;
  flat sessions keep the existing per-way lookup. Only selected ways contribute references;
  oversized selections skip prefetch.

`prepare_batch` and `prepared_session` share an immutable selected-coordinate
table across compact planet-way workers. Flat lookup ignores preparation.
`prepare_refs` is a no-op on these prepared sessions; legacy callers retain
per-blob prefetch. Source validation and output coordinate semantics are unchanged.
