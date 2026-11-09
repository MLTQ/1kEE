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

On macOS, the narrow AVFoundation bindings provide native asynchronous radio
playback, with no external audio player dependency. Other platforms use ffplay.

The buildout adapter uses a temporary file in the cache directory to publish three validated tables atomically.

LZ4 block compression and CRC32 back the bounded interactive Atlas recent-view
cache. These versions were already locked through other workspace dependencies;
frame-format dependencies and elevation quantization are not introduced.
