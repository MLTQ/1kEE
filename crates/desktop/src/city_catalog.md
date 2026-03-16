# city_catalog.rs

## Purpose
Provides a searchable local GeoNames-backed city catalog for terrain focus and cache precompute workflows. This file exists so the app can offer global city search and selection without requiring live network fetches once the official dump has been downloaded and indexed locally.

## Components

### `CityEntry`
- **Does**: Stores one searchable city record with English display fields, coordinates, population, and alias strings
- **Interacts with**: `model.rs`, `terrain_library.rs`, `terrain_precompute.rs`

### `by_id`
- **Does**: Resolves one city row from the derived GeoNames SQLite catalog by geoname id
- **Interacts with**: UI selection and precompute queue code

### `search`
- **Does**: Queries the derived GeoNames SQLite catalog against name, ASCII name, country, and alternate names, then ranks by prefix quality and population
- **Interacts with**: `terrain_library.rs`
- **Rationale**: Keeps the typeahead experience local and predictable while preferring English-friendly labels from the full downloaded city corpus

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `terrain_library.rs` | Search results are stable and include display name, country, and coordinates | Removing fields or changing search ordering drastically |
| `model.rs` | `by_id` returns consistent coordinates for manual terrain focus | Renaming ids or changing coordinates without migration |

## Notes
- The runtime source is `Derived/geonames/populated_places.sqlite`, which is expected to be built from the downloaded official GeoNames dump.
- If the derived catalog is missing, search results will be empty rather than falling back to stale bundled literals.
