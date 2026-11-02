# NCEI gravity map storage inventory

Measured November 2, 2026 from the [NCEI gravity catalog](https://www.ncei.noaa.gov/products/gravity-data),
its linked legacy file directories and the separate
[ocean gravity/roughness collection](https://www.ncei.noaa.gov/products/downward-continued-gravity-world-oceans).

## Result

All sizes below are hosted download bytes, using decimal MB/GB. Compressed
ocean files remain compressed in these totals; expanded grids/runtime caches
would occupy additional space.

| Map products | Named maps | One preferred format each | All offered numeric formats |
|---|---:|---:|---:|
| Gravity anomalies: free-air, Bouguer, isostatic, basement, downward-continued | 32 | 206,554,829 bytes (206.6 MB) | 1,944,795,854 bytes (1.945 GB) |
| Derived ocean-gravity roughness | 4 | 190,087,739 bytes (190.1 MB) | 1,598,354,534 bytes (1.598 GB) |
| Derived terraced gravity | 1 | 377,340 bytes (0.4 MB) | 4,896,810 bytes (4.9 MB) |
| **Gravity maps total** | **37** | **397,019,908 bytes (397.0 MB)** | **3,548,047,198 bytes (3.548 GB)** |
| Optional geoid/deflection fields | 17 | 47,309,372 bytes (47.3 MB) | 446,545,541 bytes (446.5 MB) |

The complete scanned directories contain **541 files / 4,460,887,455 bytes
(4.461 GB)**, including supporting documentation, reference layers, model input
tables and alternate formats. Every file returned an exact HTTP Content-Length;
all 46 visited directory listings succeeded. No products remain unclassified.

## Scope and method

- Parsed 108 catalog rows (79 distinct access URLs), selected map/model roots,
  and traversed the linked legacy grid tree plus the 2008 ocean-map directory.
- Read HTML indexes, HTTP HEAD responses and small headers/README files.
  The numerical map payloads were not downloaded.
- Header `data_type` and `file_title` distinguish gridded gravity from station
  tables, sampling-density maps, geoids, terrain/depth and other reference maps.
  A depth-to-basement grid remains a depth map even where a catalog label differs.
- One-format selection prefers original `.bin`, then geographic `.nc.gz`, then
  GMT grids. Explicitly identified IDRISI copies share the original product key.
  Distinct resolutions/interpolations/masks remain separate named maps.
- Projections and alternate representations can differ; these are product-based
  storage choices, not a proof that all encodings contain identical samples.
  Do not use a minimum-file-size rule: one ASCII export is only 35 bytes and an
  IDRISI variant is much shorter than its declared dimensions imply.
- Raw marine cruise databases and station-only catalog collections are outside
  this **map** tally. It is not a size estimate for every NCEI gravity observation.

## Examples

| File | Exact bytes |
|---|---:|
| Haxby global anomaly `.bin` | 18,662,400 |
| Same named Haxby product `.xyz` | 230,478,708 |
| North America 2.5-minute anomaly `.bin` | 15,693,696 |
| Downward-continued ocean gravity `.nc.gz` | 83,356,017 |

## Reproduce and inspect

```bash
python3 scripts/inventory_ncei_gravity.py \
  --out-dir /Volumes/Hilbert/Data/Gravity/NCEI/inventory-2026-11-02
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts \
  -p 'test_inventory_ncei_gravity.py'
```

The Hilbert directory contains `inventory.json` (every file URL, byte count,
classification evidence and chosen map representation) plus cached metadata.
Use a new dated directory for a fresh snapshot; the same directory reuses cache.
