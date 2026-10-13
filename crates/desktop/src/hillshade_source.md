# hillshade_source.rs

## Purpose
Select public USGS or Washington DNR shaded-relief imagery for the local map.
Washington's bare-earth and top-surface images are visual overlays, never inputs
to contours, marker elevation or terrain geometry.

## Components
- `Source` supplies labels, coverage screens and worker-only downloads.
- `Bounds` holds longitude/latitude image extents and texture reuse checks.
- `washington_url` targets the public DNR composite MapServer: layer 1 is bare
  earth, layer 0 is top surface. Geographic bbox/image SRs keep north-up pixels
  aligned with the local renderer. PNG32 retains transparent missing coverage.

## Contracts
- `model` defaults to USGS; `layer_drawer` selects either DNR product explicitly.
- `hillshade_layer` calls fetch only on its single background worker.
- All sources obey the existing detailed-terrain streaming setting.
- DNR transfers have a 30-second deadline, 8 MiB limit and 2048-pixel edge cap.
  Error documents never reach image decoding. Extents only screen candidates.
- DNR credits must remain visible in the layer controls. The portal supplies
  project metadata and raw-data downloads; 1kEE does not mirror those holdings.

## Verification
Public service metadata and both small Seattle products were verified on
2026-10-13. See `hillshade_source_tests` for protocol and live smoke checks.
