# platforms.rs

Parses `platforms --input platforms.json --out platforms.1ka` and delegates validation and atomic publication to tile_archive::platforms. Missing/unknown flags fail; existing destinations are never replaced.
