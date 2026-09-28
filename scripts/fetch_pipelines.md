# fetch_pipelines.py

Downloads the EIA layers hosted by DOE, BSEE offshore routes, and the GEM gas/oil
GeoJSON snapshots linked from the publisher's public maps. Uses curl, Python's
standard library, and GDAL ogr2ogr. No registration or form submission occurs.

## Contracts
- `--out-dir` names a snapshot directory. Raw downloads resume without refreshing;
  normalized snapshots refuse replacement. New dates use new directories.
- EIA pagination uses verified object-ID batches, never a single truncated query.
- Original files and SHA256/source/license provenance remain alongside routes.
- BSEE uses WGS84 conversion with no ballpark transformation; non-fuel utilities
  are excluded. Original product/status codes remain in the raw snapshot.
- JSONL contains one continuous route part per record with shared `pipelines::Info`
  metadata. Never joins MultiLineString parts or converts point terminals to lines.
- Missing status/route precision is explicitly unknown. GEM owner is not presented
  as operator. Historical and planned records remain separately filterable.
- Optional GeoJSON altitude ordinates are omitted from the horizontal route
  format, counted in normalization statistics, and preserved in raw snapshots.
- GEM data: Global Energy Monitor, GGIT November 2025 / GOIT June 2026 public maps,
  CC BY 4.0: https://globalenergymonitor.org/creative-commons-license .
