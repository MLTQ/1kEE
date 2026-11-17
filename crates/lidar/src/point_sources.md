# Target source footprints and bulk points

Actual Polygon/MultiPolygon footprints, including holes and date-line wrapping,
rank candidate surveys by center coverage and 49 radial sample locations before
date. Sampling selects candidates; actual source returns determine coverage.
The real Manhattan geometry fixture prevents a newer neighboring New Jersey
bounding box from outranking the NYC footprint with an unspecified catalog date.

For supported OpenTopography records, a trusted catalog STAC collection slug
resolves to the publisher's public bulk listing. Only complete, single-file
LAS/LAZ collections within the 32 MiB per-file limit are accepted. Large or
multi-file sources need a spatial index. Downloads use the existing bounded
cache, transfer/time limits, free-space reserve and cancellation behavior.

Rust decodes original XYZ, classifications, intensity, return number and RGB.
Noise 7/18 and withheld returns are excluded. A caller-supplied XY-only
transform uses the declared horizontal CRS; Z is retained without offsets.
Exact WGS84 radial clipping follows the transform. Budget-limited reads remain
explicitly incomplete. An undeclared vertical reference is never invented.
