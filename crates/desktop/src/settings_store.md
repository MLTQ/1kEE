# settings_store.rs

## Purpose
Persists a small amount of local desktop-app configuration that should survive restarts. Right now that means the Factal API key used for live polling.

## Components

### `load_factal_api_key`
- **Does**: Reads the locally saved Factal API key from the workspace settings file
- **Interacts with**: `model.rs`, `factal_settings.rs`

### `save_factal_api_key`
- **Does**: Writes or clears the locally saved Factal API key
- **Interacts with**: `factal_settings.rs`
- **Rationale**: Keeps the first-pass secret persistence simple while avoiding a heavier settings subsystem

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `model.rs` | Loading returns `None` when no key is stored | Returning empty strings instead of `None` |
| `factal_settings.rs` | Saving an empty key clears the on-disk value | Changing the clear semantics |

## Notes
- The key is currently stored as a plain text file in the workspace root because this is a local demo, not a hardened credential store.
- If we later need multi-user or machine-level secrets handling, this should move behind a platform keychain layer.
