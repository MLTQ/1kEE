# infra_layer.rs

Loads and draws cached local OSM infrastructure (rail, pipelines, aeroways,
military, communications, industry, ports, government and surveillance).
Each type has independent load bounds/cache state; source reads run on workers.

- `draw_infra` shares geographic projection, class styling and optional polygon
  fill. Per-type entry points select their source prefix and style.
- Pipeline line hits use the exact projected points being drawn and feed
  infrastructure_hover. Retained name, class and OSM way ID are available;
  operating status/owner/operator are not retained by this cache and are not
  invented. Other infrastructure hover behavior stays unchanged.
- Public EIA/GEM/BSEE metadata lives in independent pipeline_layer/globe modules.
- Existing cache invalidation uses root, bounds and OSM data generation.

Airport/aerodrome boundaries draw as subdued outlines. They describe the whole
airport property, not runway pavement, and are never filled. Runways/airstrips
draw more prominently. Closed runway, helipad and terminal footprints can fill
their actual source areas; open centerlines stay lines, without inferred widths
or invented closing edges. Closed runway areas remain usable even in older cells
whose polygon flag was false. No cache rebuild is required.

The loader prepares filled infrastructure using the shared `tree_geometry` Lyon
triangulator before projection. Concave courtyards stay empty; no `convex_polygon`
triangle fan is submitted. Drawing preserves triangle indices and breaks strokes
at unavailable projections. Pipeline hit distances use only drawn path segments.
Longitude unwrap and whole-feature shifts keep short source edges at the date
line. Existing infrastructure map-plane elevation placement is unchanged.

Regressions cover airport perimeter/runway separation, concave terminal and
government geometry, legacy binary runway flags, projection gaps, open rings and
date-line strips. Opt-in tests inspect and render existing Nashville airport
geometry from Minkowski without rebuilding source cells.

`draw_pipelines` accepts the capacity mode: OSM uses the shared unavailable-capacity gray because its cached attributes contain no capacity. Its hover metadata explicitly has `capacity: None`.
