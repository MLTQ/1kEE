# Local coverage and 100 km Target validation

Verified the production Local projection, Target DEM loader, native EPT reader
and GPU renderer around Kinston, NC (35.26, −77.58) on 2026-11-16. GPU:
NVIDIA RTX 4090, Vulkan, driver 615.78.08. Machine-readable results are in
[the validation receipt](target-radius-2026-11-16.json).

| Check | Result |
| --- | --- |
| Local catalog coverage | 681 surveys, 16,515 vertices; no preparation required |
| Indexed catalog read | 15.50 ms with the catalog warm |
| Local projection and egui shape creation | 5,160 clipped segments in 1.40 ms |
| Target radius | 100 km; 200 km diameter |
| Original source points | 50,000 retained; farthest return 92.3 km from center |
| DEM numeric grid | 2048 × 2048, approximately 98.1 m sample spacing |
| DEM display mesh | Approximately 392.3 m spacing; missing data stays holes |

The live Target integration test deliberately caps its point stream at 50,000
returns, reuses cached source blocks and reports the point-budget limit. It
checks that regional acquisition, circular clipping and both GPU displays work;
these results do not measure full-density regional loading or whole-app frame
time. The regular UI retains its independent 1–16 million point budget.

The coverage query uses the covering extent index before reading matching
geometries. The layer includes published surveys without requiring prepared
rasters. Per-view limits remain 2048 surveys, 250,000 vertices and 32 MiB of
geometry. Context roads and vector cells retain a central 5 km detail window
for larger Targets, while imported context stays clipped to the full radius.

Automated checks cover WGS84 bounds and round trips at 100 m, 1 km and 100 km,
including high latitudes and the date line; matching camera framing; click
entry and Escape preserving the Local camera and selected radius; unprepared
catalog polygons and holes; and unchanged fine-elevation request limits.
The existing GPU depth, classification and resource-release regression passed.
