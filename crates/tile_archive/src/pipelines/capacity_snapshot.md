# capacity_snapshot.rs / capacity_snapshot.json

Restores capacity metadata discarded by older importers. The bundled JSON is a
small adaptation of Global Energy Monitor's GGIT November 2025 and GOIT June 2026
public map releases, under [CC BY 4.0](https://globalenergymonitor.org/creative-commons-license).
Contains 2,246 gas values (including ten reported zeros) and 1,150 oil/liquids
values. Unknown entries are omitted, never replaced by the map's scaled zero.

## Contracts
- `pipelines::load` calls `enrich` on workers after reading geometry. It requires
  exact source ID/URL and original raw-file SHA256 matches in the archive manifest.
- Joins use source/project ID only within that verified release. No name or
  geometric guessing. Unmatched archives remain readable with unavailable capacity.
- Explicit archived capacities always win, including zero. No disk writes,
  downloads or changes to geometry/identity/status occur during enrichment.
- Parsing is cached once. The supplement is included in the binary; files on
  the user's data drive remain unchanged. `decode` alone reads only tile metadata.

Generated with `scripts/pipeline_capacity_snapshot.py` from the two configured
GEM URLs. Full URLs and raw SHA256 values are in the JSON. Regeneration is
deterministic and refuses output replacement; validate a new result before
replacing the checked-in snapshot. Tests check provenance mismatches, preservation
of explicit values, counts, units and finite nonnegative quantities.
