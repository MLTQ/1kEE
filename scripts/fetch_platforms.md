# fetch_platforms.py

Downloads BSEE Gulf platforms and EMODnet European offshore installations into
an immutable, dated source directory. Produces platforms.json for the Rust
archive packer; raw ZIP, GeoJSON and European license metadata stay alongside it.

- Uses public WFS with CRS84 (longitude first), verifies matched/returned counts,
  and refuses silently truncated inventories. A future >20,000-point response
  requires pagination before use.
- Transforms BSEE NAD27 to WGS84 with GDAL and no ballpark transformations.
- Validates coordinates and stable per-source IDs. Does not deduplicate nearby
  installations: a complex can have multiple legitimate structures.
- Keeps status verbatim. Missing BSEE removal dates mean unknown operating
  status; removed/closed/decommissioned and planned installations are separate.
- Subsea, buoys and terminals are retained as optional support installations,
  not represented as above-water drilling rigs. No field centroids are imported.
- Retains source attribution, download time, URLs and SHA256 checksums. Source
  publication dates can predate retrieval; these are static snapshots.
- Requires Python 3, curl and GDAL. Refuses overwrite of completed normalized data;
  interrupted downloads reuse completed raw snapshots and clean owned scratch.

BSEE identity is (COMPLEX_ID, STRUCTURE_): structure numbers alone repeat across complexes. Display names are not identity keys.
