# Offshore oil and gas installations

Enable **Layers → Infrastructure → Oil & gas platforms** after restarting the
rebuilt desktop app. Diamonds work on the globe and in local terrain views;
hover shows source, identity, facility type, reported status and available
operator/product/date/depth attributes. BSEE and EMODnet can be toggled separately.

## Installed snapshot

Raw data and provenance: `/Volumes/Hilbert/Data/Platforms/2026-09-27/`.
Runtime archive: `/Volumes/Hilbert/Derived/platforms.1ka` (3,166,208 bytes).
The directory uses the local download date; the manifest records UTC retrieval.

| Source | Records | Visible by default | Coverage |
|---|---:|---:|---|
| BSEE | 7,317 | 1,553 | Gulf federal offshore platform structures |
| EMODnet / Cogea | 1,617 | 862 | European seas, particularly Norway/UK/Netherlands |
| Total | 8,934 | 2,415 | Regional inventories, not worldwide completeness |

- Removed/inactive and construction records have separate optional filters.
- EMODnet's 529 subsea/buoy/terminal records are retained behind a support
  installation filter. Some are also historic/planned, so counts overlap.
- Missing BSEE removal dates are explicitly **unknown operating status**.
- Fixed and floating facilities use static inventory coordinates. There is no
  live mobile-rig feed, and oilfield centroids are not represented as rigs.
- A complex may contain several distinct structures; identity uses BSEE's
  complex ID plus structure number, never proximity or display name.

## Sources and attribution

- [BSEE mapping downloads](https://www.data.bsee.gov/Main/Mapping.aspx): public
  US Government platform shapefile, DBF updated 2026-09-01. Includes 5,764
  removed structures. GDAL transforms NAD27 to WGS84 without ballpark operations.
  Coordinates are approximate inventory positions, not navigational data.
- [EMODnet inventory metadata](https://emodnet.ec.europa.eu/geonetwork/srv/eng/catalog.search#/metadata/ddbe3597-4e3f-4e74-8d31-947c4efef2e9):
  EMODnet Human Activities / Cogea Srl, **CC BY 4.0**. Downloaded through the
  advertised WFS using CRS84 and verified matched/returned totals. Metadata
  revised 2025-09-02; underlying source dates vary. Retrieval does not imply a
  fresh operational survey. The original metadata XML is retained for attribution.

Raw downloads total ~6 MB including normalized data and metadata. Source files
are hashed in platforms.json. Runtime reads need only the .1ka file.

## Refresh and world packing

```sh
python3 scripts/fetch_platforms.py --out-dir /path/to/new-dated-snapshot
one-thousand-electric-eye-cache-builder platforms \
  --input /path/to/new-dated-snapshot/platforms.json \
  --out /path/to/new-platforms.1ka
one-thousand-electric-eye-cache-builder pack-archive \
  --platforms /Volumes/Hilbert/Derived/platforms.1ka \
  --out /path/to/new-world.1ka
```

Both builders refuse existing destinations. For mixed world packs add the
normal OSM/contour/pipeline arguments. GUI and CLI packs with `--osm-cache-dir`
automatically include a sibling platforms.1ka. Existing world.1ka is unchanged.
The renderer prefers the standalone inventory, then the RIGS namespace in world.
Use Reload after replacing a snapshot while the app is running.

## Runtime and verification

- RIGS/grid 4 stores a versioned, CRC-checked small point inventory with source
  metadata. World packing validates and copies its original bytes exactly.
- Disk reads and JSON decoding run on a worker; visibility toggles reuse data.
  Batched screen meshes rebuild on camera/filter changes and are reused at rest.
- Default 2,415 diamonds took ~0.68 ms to prepare in a debug real-data test
  (simple projection, not an end-to-end frame benchmark). All 8,934 records
  loaded/validated in ~78 ms on a worker, also in debug mode.
- Workspace tests, source-normalization regressions and release builds pass.
  Archive tests cover IDs/coordinates/status filters, publication refusal,
  namespace preservation and exact world-copy fidelity. Real-data validation
  reconciles all record and filter counts with the saved manifest.
- Live visual verification was unavailable because the Mac was locked.
