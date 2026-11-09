# atlas_relief.rs

## Purpose
Own the Atlas overview and independently refined Globe/Local relief images.
Source discovery, decoding, lighting and texture preparation stay on coalesced
background workers; painting never opens terrain files or invokes GDAL.

## Components
- `load_grid` reads the small existing GEBCO overview as an immediate fallback.
- `detail_texture` uses `atlas_viewport` and `atlas_source` to load native windows
  up to the live GPU limit (8192 edge / 32 million pixels maximum). Root-keyed
  snapshots retain each previous georeferenced window while replacements load.
- `draw_globe` paints overview then native relief on the unit sphere. Regional
  tessellation follows camera scale. The actual scene rectangle drives selection.
- `draw_local` uses the inverse oblique clip for fine detail; the overview still
  unwraps longitude quarters independently around the dateline.
- `paint_cached` retains four immutable Arc meshes (overview/detail × two views),
  keyed by texture and all camera/layout inputs. `clear` releases these plus all
  image snapshots and rejects stale workers.

## Contracts
Only Earth + Atlas paints this base. Contours and every other layer retain their
independent visibility controls; no theme selection changes layer choices.
Scientific images, optional contours and operational markers draw above relief.
No overview file is required for the original-data detail path to start.
Missing sources leave the existing image visible and retry with snapshot backoff.
