# saved.rs

## Purpose
Persists up to 200 user-added, geographically pinned public stream URLs beside
the executable, in `.1kee_radio_feeds.json`. No provider keys or audio are saved.

## Components
- `load` bounds input size and retains only valid named/geolocated public feeds.
- `save` writes a temporary JSON file then renames it over the previous list.
- Reads and writes enforce a 1 MiB limit before allocating or replacing user data.

## Contracts
| Dependent | Expects | Breaking changes |
|---|---|---|
| Radio panel | Save errors preserve the previous in-memory list | Silently forgetting a failed save |
| User configuration | Stream list remains separate from settings and API keys | Writing to the data drive without an explicit path |
