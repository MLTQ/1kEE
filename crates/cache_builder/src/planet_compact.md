# planet_compact.rs

## Purpose
Runs the opt-in compact Build All path through the existing feature pipeline,
keeping all state under `tmp_dir/indexed-pbf-v1` instead of using legacy scratch.

## Contracts
- Existing `planet_nodes.bin`, sparse-index files and `checkpoint.txt` are never
  opened, reset, migrated or deleted. First compact run scans the original PBF.
- Its index binds to source identity; feature resume/completion also binds to
  canonical output/elevation paths, selected layers and pipeline version.
- Repeated completed runs are no-ops for the same settings. Use a new working
  directory to rebuild deleted output or change settings/source.
- Index scanning reports byte-based progress and actual index size. Worker
  sessions retain up to 16 MiB of decoded blocks plus up to 12 MiB of grouped
  way dependencies; scratch vectors and the active decoder add bounded memory.
- Uses the existing classification, elevation baking, cell merging and feature
  checkpoint cadence; it does not extend planet-all's missing admin support or
  solve the separately tracked cell-output durability/coverage issues.
- Index errors include their working-directory path. The completion summary
  reports feature/file counts without relying on the legacy partial cell tally.
