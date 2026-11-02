# fetch_swot.py

## Purpose
Download and prepare Scripps SWOT_06 marine vertical gravity gradients for the
optional desktop layer. Requires Python 3.11+, curl and GDAL; no credentials.

## Components
- `fetch` resumes partial transfers and only publishes complete downloads.
- `validate_source` rejects unexpected product dimensions, registration or
  packed values before any conversion.
- `prepare` quantizes physical Eötvös to Int16×10, registers a global native
  one-arcminute grid and produces a small area-average overview.
- `check_grid` validates dimensions, registration, nodata and byte order.
- `main` downloads source/release notes and publishes the prepared directory.

`--asset-root` is required and must already exist, preventing an unmounted drive
from redirecting a large download onto the system disk. Defaults under that root
are `Data/SWOT/SWOT_06` and `Derived/swot`; explicit directory overrides exist.
The user's installation uses `/Volumes/Hilbert`.

## Contracts
Source is `curv_SWOT_06.nc`, dated September 15, 2026, extent ±80° latitude.
Missing cells/polar caps stay −32768; zero gravity is valid. The source's finite
reference-model values over land are retained; no additional land mask is applied.
Outputs are north-up global BIL grids, 21600×10800 and 4096×2048,
little-endian Int16, 0.1 Eötvös
per stored integer. `swot_layer/grid.rs` consumes exactly this contract.
SHA256/source URLs/retrieval date and both requested citations are preserved in
a manifest; release notes accompany the output. Raw/derived data is gitignored.
Output is published by directory rename only after all checks pass; existing
completed outputs are never replaced. Owned scratch is removed on failures.
The original paper's archived dataset is separate from this newer product.
