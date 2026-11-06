# ncei_gravity_maps.py

## Purpose
Turn the NCEI file inventory into a map-specific storage tally using header
titles/data types rather than catalog labels or directory names alone.

## Components
- `canonical` merges explicitly identified IDRISI convenience copies with their
  original map names. Names with different map resolutions remain separate.
- `title_and_type`, `category` distinguish anomaly and derived gravity maps,
  geodetic fields, sampling density, terrain/reference maps, observations and
  model inputs. Point files remain observations even in a grids directory.
- Ocean-gravity product descriptions come from the published 2008 README when
  individual header files are absent; the exact source URL is retained.
- `preferred_rank` selects binary originals before geographic NetCDF and GMT
  grids. It never chooses a tiny ASCII placeholder simply because it is small.
- `map_summary` retains per-product evidence, chosen URLs, alternate formats,
  exact byte totals by category and placeholder-sized variants.

## Contracts
Unknown products remain explicitly unclassified. Totals describe catalog-linked
legacy products, not all NOAA observations. Alternate projections/representations
can differ; grouping is based on product identity, not a bytewise equivalence
claim. Inventory metadata is preserved so classification can be audited.
