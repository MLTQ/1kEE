# city_catalog.rs

## Purpose
Provides a bundled searchable city catalog for terrain focus and cache precompute workflows. This file exists so the app can offer global city search and selection without requiring a live network fetch at runtime.

## Components

### `CityEntry`
- **Does**: Stores one searchable city record with English display fields, coordinates, population, and alias strings
- **Interacts with**: `model.rs`, `terrain_library.rs`, `terrain_precompute.rs`

### `all` / `by_id`
- **Does**: Expose the bundled city records and resolve individual entries by stable id
- **Interacts with**: UI selection and precompute queue code

### `search`
- **Does**: Filters and ranks cities against the user's free-text query using name, ASCII name, country, and aliases
- **Interacts with**: `terrain_library.rs`
- **Rationale**: Keeps the typeahead experience local and predictable while preferring English-friendly labels

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `terrain_library.rs` | Search results are stable and include display name, country, and coordinates | Removing fields or changing search ordering drastically |
| `model.rs` | `by_id` returns consistent coordinates for manual terrain focus | Renaming ids or changing coordinates without migration |

## Notes
- The current dataset is bundled directly in source as a practical bootstrap.
- The structure is designed so a future GeoNames-derived import can replace the literal list without changing the UI or precompute contracts.
