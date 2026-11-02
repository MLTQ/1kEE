# inventory_ncei_gravity.py

## Purpose
Measure the publicly linked NCEI gravity map footprint without downloading
numeric datasets. Retain directory listings and HEAD metadata for audit/reuse.

## Components
- `Tables`, `catalog_products`, `listing` parse Drupal catalog and Apache index
  tables. Child traversal rejects parent, sorting and external/navigation links.
- `Metadata` caches bounded metadata GETs and size-only HEADs, limits request
  starts to five per second across four workers, and records missing sizes.
- `crawl` follows selected legacy map roots with a 500-directory guard; software
  and duplicate listing-document subtrees are excluded explicitly.
- `measure` reads Content-Length with identity encoding. Approximate Apache
  listing sizes remain separate and never silently enter exact byte totals.
- `describe` reads only small (≤64 KiB) header/readme files after HEAD confirms
  their size, retaining enough provenance to classify actual mapped quantities.
- `numeric_key`, `summarize` group named numeric basenames across formats;
  the smallest-format sum is a planning comparison, not content deduplication.
- `main` preserves all catalog entries, selected scope, excluded/external
  targets, every inventory row, errors and summary in `inventory.json`.

## Contracts
Use an explicit output directory on Hilbert. Only public HTTP metadata and
directory pages are read, with no authentication, order submission or dataset
payload downloads. Totals with missing headers or directory failures are lower
bounds. The crawl includes geoid/density/terrain references mixed into the
gravity grid tree. `ncei_gravity_maps` uses inspected headers to separate those
from anomaly maps. The linked 2008 ocean gravity/roughness directory is also
included. No scientific maps are imported here.
Reusing an output directory reuses its cached snapshot; choose a new dated
directory when fresh HTTP measurements are required.
