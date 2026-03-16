# factal_settings.rs

## Purpose
Renders the small settings window used to configure Factal authentication for the desktop app. This file is the user-facing control surface for storing, clearing, and manually refreshing the private API key.

## Components

### `render_factal_settings`
- **Does**: Draws the Factal API window, persists the configured key, triggers live refreshes, and reports failures to the activity log
- **Interacts with**: `AppModel` in `model.rs`, `settings_store.rs`, `factal_stream.rs`, theme helpers in `theme.rs`
- **Rationale**: Keeps secret entry and live-stream control separate from the data-root, terrain, and city-library controls in the rest of the shell

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | Calling the renderer is enough to keep the settings window functional | Renaming or removing the render entrypoint |
| `header.rs` | The window opens when `factal_settings_open` is toggled on the model | Ignoring the open flag or relocating the window state |
| `settings_store.rs` | Saving an empty string clears the stored key | Changing save semantics for empty values |

## Notes
- The key is masked in the UI but still stored as plain text because this is still a local demo build.
