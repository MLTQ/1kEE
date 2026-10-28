# admin_geometry.rs

## Purpose
Prepare immutable region meshes on the admin loader worker using Lyon even-odd
fill. Concave borders, islands and holes must retain their topology.

## Contracts
- `tessellate` rejects open rings. Callers require all relation members before
  requesting fill; old line-only caches have no completeness guarantee.
- `unwrap_rings` keeps consecutive longitudes continuous at the dateline and
  aligns components near one anchor. Local painting shifts whole regions by 360°.
- Fill is computed once per snapshot, not on each frame or slider adjustment.
- Tests cover concavity, holes, multiple islands, open rings and dateline area.
