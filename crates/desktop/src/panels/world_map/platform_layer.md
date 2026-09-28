# platform_layer.rs

Loads the small public offshore inventory from Derived/platforms.1ka, falling
back to the RIGS namespace of world.1ka. Disk access/JSON validation run on a
worker. No network access or planet parsing happens during map painting.

- A root-keyed immutable snapshot survives visibility toggles. Reload/root
  switches discard the old channel so stale workers cannot publish results.
  Errors remain visible in controls until reload; no per-frame read retries.
- Batched diamond meshes cache by revision, full projection inputs, clip,
  palette and all filters. Only visible finite projected positions enter hit
  testing. Every eligible point is drawn; no feature-count budget.
- Initial source loading is asynchronous. Reprojection of ~9,000 points is
  lightweight CPU work only when the camera/filter changes; idle meshes reuse.
- Globe caller enforces the perspective sphere horizon. Local caller projects
  at sea level through the same transform as terrain/route overlays. Earth only.
- Hover shows source, reported status, facility kind and available attributes;
  it never equates inventory locations or unknown status with live operation.
- Defaults hide historic/planned/subsea/support records. Sources may contain
  multiple structures in a complex; nearby points are not merged arbitrarily.
- UI source access uses try_lock, with no disk or heavy mesh work under mutex.

Visible diamonds feed the common infrastructure_hover card. Platforms take priority over coincident pipeline lines; nearby installation identities are retained in overlap summaries.
