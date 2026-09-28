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
