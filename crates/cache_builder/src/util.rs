#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoPoint {
    pub lat: f32,
    pub lon: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoBounds {
    pub min_lat: f32,
    pub max_lat: f32,
    pub min_lon: f32,
    pub max_lon: f32,
}

#[derive(Clone, Debug)]
pub struct RoadPolyline {
    pub way_id: i64,
    pub road_class: String,
    pub name: Option<String>,
    pub points: Vec<GeoPoint>,
}

#[derive(Clone, Debug)]
pub struct WayFeature {
    pub way_id: i64,
    pub feature_class: String,
    pub name: Option<String>,
    pub points: Vec<GeoPoint>,
    pub is_polygon: bool, // true = close ring for Polygon geometry
}

pub fn canonical_road_class(value: &str) -> Option<&'static str> {
    match value {
        "motorway" | "motorway_link" => Some("motorway"),
        "trunk" | "trunk_link" => Some("trunk"),
        "primary" | "primary_link" => Some("primary"),
        "secondary" | "secondary_link" => Some("secondary"),
        "tertiary" | "tertiary_link" => Some("tertiary"),
        "residential" | "living_street" | "unclassified" | "service" => Some("minor"),
        _ => None,
    }
}

pub fn canonical_waterway_class(value: &str) -> Option<&'static str> {
    match value {
        "river" | "canal" => Some("river"),
        "stream" | "drain" | "ditch" => Some("stream"),
        _ => None,
    }
}

pub fn canonical_building_class(value: &str) -> Option<&'static str> {
    if value.is_empty() || value == "no" {
        None
    } else {
        Some("building")
    }
}

pub fn canonical_tree_class(key: &str, value: &str) -> Option<&'static str> {
    match (key, value) {
        ("natural", "wood") | ("landuse", "forest") => Some("forest"),
        _ => None,
    }
}

pub fn expand_bounds(bounds: GeoBounds, margin_degrees: f32) -> GeoBounds {
    GeoBounds {
        min_lat: (bounds.min_lat - margin_degrees).clamp(-85.0511, 85.0511),
        max_lat: (bounds.max_lat + margin_degrees).clamp(-85.0511, 85.0511),
        min_lon: (bounds.min_lon - margin_degrees).clamp(-180.0, 180.0),
        max_lon: (bounds.max_lon + margin_degrees).clamp(-180.0, 180.0),
    }
}

pub fn point_in_bounds(point: GeoPoint, bounds: GeoBounds) -> bool {
    point.lat >= bounds.min_lat
        && point.lat <= bounds.max_lat
        && point.lon >= bounds.min_lon
        && point.lon <= bounds.max_lon
}

pub fn polyline_bounds(points: &[GeoPoint]) -> GeoBounds {
    let mut min_lat = f32::INFINITY;
    let mut max_lat = f32::NEG_INFINITY;
    let mut min_lon = f32::INFINITY;
    let mut max_lon = f32::NEG_INFINITY;
    for point in points {
        min_lat = min_lat.min(point.lat);
        max_lat = max_lat.max(point.lat);
        min_lon = min_lon.min(point.lon);
        max_lon = max_lon.max(point.lon);
    }
    GeoBounds {
        min_lat,
        max_lat,
        min_lon,
        max_lon,
    }
}

pub fn bounds_intersect(left: GeoBounds, right: GeoBounds) -> bool {
    left.min_lat <= right.max_lat
        && left.max_lat >= right.min_lat
        && left.min_lon <= right.max_lon
        && left.max_lon >= right.min_lon
}

pub fn focus_cells_for_bounds(bounds: GeoBounds) -> Vec<(i32, i32)> {
    let min_lat_c = bounds.min_lat.floor() as i32;
    let max_lat_c = bounds.max_lat.floor() as i32;
    let min_lon_c = bounds.min_lon.floor() as i32;
    let max_lon_c = bounds.max_lon.floor() as i32;
    (min_lat_c..=max_lat_c)
        .flat_map(|lat| (min_lon_c..=max_lon_c).map(move |lon| (lat, lon)))
        .collect()
}

pub fn focus_cell_bounds(cell_lat: i32, cell_lon: i32) -> GeoBounds {
    GeoBounds {
        min_lat: cell_lat as f32,
        max_lat: (cell_lat + 1) as f32,
        min_lon: cell_lon as f32,
        max_lon: (cell_lon + 1) as f32,
    }
}
