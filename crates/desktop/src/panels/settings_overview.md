# settings_overview.rs

## Purpose
Keep the former banner's source status, record counts, runtime source paths,
focus information and camera-discovery progress available in Settings → Overview.

## Contracts
- Source values come from AppModel and existing inventories, without implicit opt-in.
- Enable live cameras reuses the former explicit header action.
- Background OSM notes and camera progress live here so arbitrary status strings
  cannot expand the main rail. Opening this section does not run a source poll.
