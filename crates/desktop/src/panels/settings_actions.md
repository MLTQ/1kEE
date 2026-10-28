# settings_actions.rs

## Purpose
Preserve explicit settings save, key clearing, live polling and camera discovery
behavior independently of settings navigation and layout.

## Contracts
- Save normalizes existing key/path/scope/rate fields and retains source invalidation.
- Camera discovery remains opt-in and persists its enablement before invalidation.
- Clearing keys preserves independently enabled public-directory discovery.
- No settings action runs simply because the window or a section was opened.
