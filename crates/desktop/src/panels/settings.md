# settings.rs

## Purpose
Own the single Settings window with a persistent section navigator: Overview,
Appearance, Data Sources, Files & Cache, Terrain Library, and Activity.

## Contracts
- `AppModel::settings_open` is the only global-settings window flag.
- Sections render inside this window; Terrain Library never opens another window.
- Window is available in normal and cinematic mode; Focus capture hides it while
  preserving its open state. The map rail is the shared entry point.
- Each section has its own scroll position and a bounded viewport. The action
  footer remains outside scrolling; credentials and path save semantics stay intact.
- Saving/polling/clearing delegates to settings_actions after content rendering.

Headless chrome integration tests exercise responsive layout and pointer-driven
navigation; an opt-in GPU preview exports real controls for visual inspection.

The navigation divider has an explicit height so egui cannot stretch the horizontal
row to the whole window and clip the fixed action footer.
