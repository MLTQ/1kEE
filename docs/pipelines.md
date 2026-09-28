# Public oil and gas pipelines

Enable **Layers → Infrastructure → Pipelines**. Public routes appear in both
globe and local views. Product toggles separate gas, oil, and liquids/mixed;
source toggles select EIA, GEM and BSEE. Planned/construction and historical
routes are hidden by default. Unspecified status remains visible and is labeled
as such. Local-view hover shows the source ID, name, owner/operator and status.
The existing OSM pipeline layer remains independently toggleable in local view.

## Installed snapshot

- Original downloads and SHA256 provenance:
  `/Volumes/Hilbert/Data/Pipelines/2026-09-27/manifest.json`.
- Runtime cache: `/Volumes/Hilbert/Derived/pipelines.1ka`, 266,027,008 bytes
  (253.7 MiB), 46,702 quarter-degree detail tiles plus one globe overview.
- 52,768 source line features, often containing multiple disconnected parts.
  5,556,030 source vertices; 504,992 overview vertices after simplification.
  Coincident/zero-length segments have no drawable geometry.
- Sources overlap intentionally; IDs remain namespaced. No name-based dedup
  silently deletes parallel pipelines or project phases. Disable individual
  sources to inspect their coverage independently.

## Provenance and limitations

| Source | Snapshot used | Data and rights |
|---|---|---|
| EIA, hosted by DOE NETL | Downloaded September 27, 2026; source vintage not established | [Public service](https://arcgis.netl.doe.gov/server/rest/services/Hosted/EIA_pipeline_data/FeatureServer): natural gas, crude oil, petroleum products, HGL. Geometry is generalized in places; absent status is unknown. |
| BSEE | September 1, 2026 mapping export | [Offshore pipelines](https://www.data.bsee.gov/Main/Mapping.aspx), US Government data. Includes active and historical fuel lines; non-fuel umbilicals, cables, water and chemical lines excluded. NAD27 transformed to WGS84 with GDAL, no ballpark transformations. |
| Global Energy Monitor | GGIT November 2025; GOIT June 2026 public map snapshots | [Gas tracker](https://globalenergymonitor.org/projects/global-gas-infrastructure-tracker), [oil tracker](https://globalenergymonitor.org/projects/global-oil-infrastructure-tracker), [CC BY 4.0](https://globalenergymonitor.org/creative-commons-license). Public-map geometry and metadata, not the complete downloadable tracker release. Routes may be approximated from endpoints. |

GEM data are adapted by clipping routes into tiles and simplifying the overview.
Map exports do not provide a reliable per-route precision field; every GEM route
retains that caveat rather than inferring accuracy from vertex count. Original
coordinates/fields remain in the downloads, including optional altitude values
that are not interpreted as pipeline burial depth or terrain elevation. Local
lines currently use the existing zero-elevation infrastructure projection.
This is geographic network coverage, not a complete local distribution network.

## Rebuild and pack

Use a fresh snapshot directory and new archive destination; commands refuse to
overwrite completed outputs. Raw downloads in an unfinished directory resume.

```sh
python3 scripts/fetch_pipelines.py --out-dir /path/Data/Pipelines/NEW-SNAPSHOT
target/release/one-thousand-electric-eye-cache-builder pipelines \
  --input /path/Data/Pipelines/NEW-SNAPSHOT/routes.jsonl \
  --manifest /path/Data/Pipelines/NEW-SNAPSHOT/manifest.json \
  --out /path/Derived/pipelines-new.1ka
```

The downloader requires curl, Python 3.11+ and GDAL ogr2ogr. It verifies EIA
object-ID batches and retains the raw public snapshots and checksums. GEM URLs
are pinned to the publisher's linked map releases; there is no automatic refresh.

The desktop reads `Derived/pipelines.1ka` first, then the public FUEL namespace in
`world.1ka`. Use **Reload pipeline archive** after installing a replacement.
Normal runtime needs only the archive, with no JSON/PBF/network access.

`pack-archive --pipelines FILE` includes these tiles and provenance in a new
world snapshot. GUI vector packs automatically include `pipelines.1ka` beside
the selected `osm` directory. This does not change an existing `world.1ka` and
does not require rebuilding any OSM or contour sources. Public FUEL tiles occupy
their own namespace, separate from OSM PIPE.

## Verification and load behavior

All 46,702 installed detail tiles passed checksum/attribute/coordinate-bound
validation. Four release-mode sample reads on Hilbert returned:

| View | Parts | Read + decode |
|---|---:|---:|
| Houston | 145 | 0.38 ms |
| Offshore Louisiana | 75 | 0.20 ms |
| Netherlands | 61 | 0.52 ms |
| Tokyo | 74 | 1.63 ms |
| World overview | 230,490 | 199 ms |

These are one-run, warmed filesystem measurements, not cold-cache or end-to-end
frame latency. Globe geometry is converted once on a worker into immutable GPU
instances; orbit/zoom changes only uniforms. The default filters produced
244,778 GPU segments (6.5 MiB), prepared once in 9.0 ms in the real-data test. Local reads/decoding happen on a
worker; ready local geometry reprojects while panning and reuses meshes at rest.
No feature-count budget truncates lines. Different source precision can still
make networks appear incomplete or misaligned.

```sh
ONEKEE_PIPELINE_ARCHIVE=/path/Derived/pipelines.1ka \
  cargo test --release -p tile-archive verify_real_pipeline_archive_and_measure_reads \
  -- --ignored --nocapture
python3 -m unittest discover -s scripts -p 'test_fetch_pipelines.py'
```
