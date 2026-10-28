# Cargo.toml

## Purpose
Declares the desktop application and its rendering, data-source and storage
dependencies. Local workspace crates provide shared tile formats and services.

## Contracts
`csv` parses user-downloaded TSV inventories with proper quoting and column
validation. Network clients, GDAL tools and the existing render stack keep their
existing features. Lockfile changes accompany dependency changes.

The cache-builder workspace library supplies the in-process administrative PBF
importer. Its narrow boundaries API runs only on an explicit background job;
normal rendering and cache reloads do not launch the companion or scan PBF files.
