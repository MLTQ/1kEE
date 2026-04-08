# camera_list.rs

## Purpose
Presents the right sidebar for nearby cameras and map-item sources. It now owns both the camera inspection tab and the ArcGIS/items tab while keeping the sidebar width bounded so long source names cannot overrun the whole app shell.

## Components

### `render_camera_list`
- **Does**: Builds the right sidebar shell, persists the active tab, and constrains the resizable sidebar width to a sane range
- **Interacts with**: `tab_cameras`, `tab_items`, `AppModel`

### `tab_cameras`
- **Does**: Shows the nearest nearby cameras with provider, type, distance, health, and action buttons, and reports when the list is capped for performance
- **Interacts with**: `AppModel::nearby_cameras`, `select_camera`, and `attempt_connect` in `model.rs`

### `tab_items`
- **Does**: Manages ArcGIS FeatureServer source input plus the enabled-layer list for imported map items, wrapping long source labels and layer names to the current sidebar width
- **Interacts with**: `arcgis_source.rs`, `AppModel::arcgis_sources`, `AppModel::arcgis_features`

## Contracts

| Dependent | Expects | Breaking changes |
|-----------|---------|------------------|
| `app.rs` | Function renders the full right sidebar and can mutate camera/item selection state | Changing the entrypoint signature or removing the sidebar shell |
| `arcgis_source.rs` callers | Long source names and layer labels stay contained within the sidebar instead of forcing panel growth | Removing width clamps or reintroducing unbounded horizontal rows |
