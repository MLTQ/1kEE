# lib.rs

Owns the shared cache construction modules and companion CLI/GUI dispatch.
The executable calls `run`; the desktop calls the narrow `boundaries` API on a
worker. Moving module ownership here avoids separate importer implementations
and keeps the existing tests in one library target. CLI syntax is unchanged.

`admin-lods <admin_cells directory>` prewarms the same per-level archives used by the desktop and reports segment counts by tier.

The `atlas` subcommand runs the same offline height builder as the Atlas GUI tab
and streams tile progress to stdout. It does not require launching the desktop.
