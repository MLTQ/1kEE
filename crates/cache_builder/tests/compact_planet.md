# compact_planet.rs

## Purpose
Exercises the public CLI with ordinary and dense-node PBF fixtures, validating
the complete compact build rather than only the lookup implementation.

## Contracts
- Flat and compact paths emit identical cell bytes for the tiny fixtures (one
  feature per file avoids unordered-map serialization differences).
- Compact mode preserves pre-existing flat node, sparse-index and checkpoint
  sentinels, and repeats completed runs without changing state or output.
- Changed feature settings or sources fail while preserving saved compact state.
- Corrupt input cannot create a feature checkpoint; invalid mode fails parsing.
- All mutations and cleanup are confined to unique owned temporary directories.
