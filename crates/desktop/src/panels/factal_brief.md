# factal_brief.rs

## Purpose
Renders the operator-facing Factal detail window for the currently selected live event. This keeps the raw/private-source payload display separate from the main map shell and event list.

## Components

### `render_factal_brief`
- **Does**: Shows a small window with the selected event headline, parsed Factal fields, and a collapsible pretty-printed raw payload when a Factal-backed event is selected and the brief window is open
- **Interacts with**: `AppModel` in `model.rs`, `FactalBrief` carried on `EventRecord`
- **Rationale**: Keeps the high-signal normalized event UI lightweight while still giving the operator access to the full payload for inspection

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | This renderer can be called every frame and only opens a window when `factal_brief_open` is set | Removing the function or changing its state contract |
| `world_map.rs` | The selected event can expose an optional `FactalBrief` payload | Removing `EventRecord::factal_brief` without replacing the detail source |

## Notes
- Non-Factal demo events close the window automatically because they do not carry a raw Factal payload.
- The raw JSON is shown read-only so the window is an inspection surface, not an editor.
