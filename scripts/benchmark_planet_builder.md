# benchmark_planet_builder.py

## Purpose
Reproduces before/after `planet-all` measurements without writing to an existing
cache or node store. Supply a representative PBF extract and two release binaries.

## Components
- `main`: alternates both binaries for 1–10 rounds, with all feature switches
  enabled and optional elevation baking disabled. Fresh temporary output is
  removed after each run; elapsed time excludes verification and cleanup.
- `cell_signature`: hashes cell identities, chunk tags, feature metadata, names,
  polygons, geometry and optional elevation bytes. Sorts encoded feature records
  to disregard nondeterministic HashMap order, retaining duplicates and every bit.

## Contracts
| Input/output | Expects |
|---|---|
| `--before`, `--after` | Existing compatible cache-builder executables |
| `--source` | Read-only PBF input; the entire supplied extract is processed |
| JSON lines | One verified run with elapsed seconds and geometry counts/hash |
| Parity | Any differing geometry/metadata/counts fail the comparison |

## Usage
`python3 scripts/benchmark_planet_builder.py --before /tmp/builder-before
--after target/release/one-thousand-electric-eye-cache-builder --source /path/to/extract.osm.pbf`
(one command line).

Results are headless and depend on source locality, selected layers, cache warmth,
and the output volume. They are not forecasts for a complete planet run.
