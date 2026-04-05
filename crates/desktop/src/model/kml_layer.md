# kml_layer.rs

## Purpose
Parses KML placemarks into the desktop app’s shared uploaded-layer geometry model. It exists to keep XML/KMZ-specific parsing out of `geojson_layer.rs` while letting the rest of the app continue to render one normalized overlay type.

## Components

### `parse_kml_features`
- **Does**: Parses an XML KML document and returns all supported placemark geometries as normalized features
- **Interacts with**: `GeoJsonLayer::parse_kml` in `geojson_layer.rs`

### `parse_placemark_geometries` / `parse_geometry_element`
- **Does**: Walks placemark children, expands `MultiGeometry`, and filters the supported geometry set down to points, lines, and polygons
- **Interacts with**: KML DOM nodes from `roxmltree`

### `parse_point` / `parse_polygon` / `parse_coordinates_node` / `parse_coordinates`
- **Does**: Convert KML coordinate text and boundary nodes into `GeoPoint` vectors
- **Interacts with**: `GeoJsonGeometry` and `GeoPoint` in `geojson_layer.rs` / `geo.rs`

### `direct_child_text`
- **Does**: Extracts direct child text such as placemark names without pulling in nested description markup
- **Interacts with**: `parse_kml_features`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `geojson_layer.rs` | Returns zero-format-specific feature types and user-facing error strings when no supported placemarks exist | Returning raw XML nodes or silently succeeding with an empty layer |
| Uploaded layer importer | Supports the GhostMaps-style KML/KMZ placemark subset used for points, lines, polygons, and multigeometries | Dropping any of those geometry handlers without updating importer messaging |

## Notes
- This parser intentionally ignores icon/style fidelity and unsupported KML constructs such as ground overlays. The current renderer only consumes vector placemark geometry.
