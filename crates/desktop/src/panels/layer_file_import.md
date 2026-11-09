# layer_file_import.rs

## Purpose
Read uploaded GeoJSON/KML/KMZ files or a projected satellite GeoTIFF on a
single worker, keeping map interaction responsive. The file extension selects
the vector parser or GDAL image loader. Successful GeoTIFF imports get their
own texture and controls in Imported Layers.

## Contracts
The explicit file picker supplies the path. While an import runs the import
button shows its loading state. The app polls a channel even in OBS mode,
applying the completed layer and existing success/error log messages exactly
once. Worker failure releases the gate. Files never parse on the drawing thread.

`button` owns the explicit picker and busy state in the Layers sidebar. Existing
asynchronous parsing and app-level completion polling are unchanged.
