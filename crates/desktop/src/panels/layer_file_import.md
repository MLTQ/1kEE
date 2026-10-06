# layer_file_import.rs

## Purpose
Read uploaded GeoJSON/KML/KMZ files and decompress/parse their content on a
single worker, keeping map interaction responsive.

## Contracts
The explicit file picker supplies the path. While an import runs the import
button shows its loading state. The app polls a channel even in OBS mode,
applying the completed layer and existing success/error log messages exactly
once. Worker failure releases the gate. Files never parse on the drawing thread.
