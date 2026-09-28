# pipeline_layer.rs

Reads public pipelines from Derived/pipelines.1ka, falling back to the FUEL
namespace in world.1ka. The globe GPU module shares this reader and owns its own immutable snapshot.
No downloads, source JSON parsing or PBF scans happen during rendering.

## Contracts
- Disk reads, attribute decoding and initial mesh preparation run on workers.
  Ready local snapshots reproject while panning and reuse meshes at rest.
  Close-up requests include a spatial margin; the globe module uses baked LOD.
- Mesh keys include root, bounds, projection, filters and palette. Only matching
  geometry paints. Old root/view jobs cannot draw into a different view.
- Root changes and Reload clear eligibility; generation checks reject pre-reload
  jobs. Toggling visibility off preserves cache, so repeated toggles stay cheap.
- Never bridges hidden globe points or disconnected line parts. Historical and
  planned lines are filtered by default, then dimmed when explicitly included.
- Hover details retain source ID, product, unknown status, owner vs operator and
  precision caveats. Labels do not claim approximate paths are surveyed routes.
- Source/product/status controls live here; the drawer delegates to `controls`.
- No feature count budget silently truncates geometry. Loading new local
  coverage is asynchronous; stale projected geometry is never substituted.
- UI reads use a nonblocking mutex attempt, clone the prepared snapshot handle,
  and release the lock before painting. Workers drop retired large snapshots
  outside the mutex so mesh replacement cannot block the UI on deallocation.

Local hits feed infrastructure_hover instead of painting their own Area. Repeated tile fragments deduplicate by source ID; overlapping routes share one card with source/accuracy metadata.
