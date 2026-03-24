use crate::model::GeoPoint;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

use super::GeoBounds;

pub(super) fn polyline_bounds(points: &[GeoPoint]) -> GeoBounds {
    let mut bounds = GeoBounds {
        min_lat: f32::INFINITY,
        max_lat: f32::NEG_INFINITY,
        min_lon: f32::INFINITY,
        max_lon: f32::NEG_INFINITY,
    };
    for point in points {
        bounds.min_lat = bounds.min_lat.min(point.lat);
        bounds.max_lat = bounds.max_lat.max(point.lat);
        bounds.min_lon = bounds.min_lon.min(point.lon);
        bounds.max_lon = bounds.max_lon.max(point.lon);
    }
    bounds
}

pub(super) fn bounds_intersect(left: GeoBounds, right: GeoBounds) -> bool {
    left.max_lat >= right.min_lat
        && left.min_lat <= right.max_lat
        && left.max_lon >= right.min_lon
        && left.min_lon <= right.max_lon
}

pub(super) fn point_in_bounds(point: GeoPoint, bounds: GeoBounds) -> bool {
    point.lat >= bounds.min_lat
        && point.lat <= bounds.max_lat
        && point.lon >= bounds.min_lon
        && point.lon <= bounds.max_lon
}

pub(super) fn expand_bounds(bounds: GeoBounds, margin_degrees: f32) -> GeoBounds {
    GeoBounds {
        min_lat: (bounds.min_lat - margin_degrees).clamp(-85.0511, 85.0511),
        max_lat: (bounds.max_lat + margin_degrees).clamp(-85.0511, 85.0511),
        min_lon: (bounds.min_lon - margin_degrees).clamp(-180.0, 180.0),
        max_lon: (bounds.max_lon + margin_degrees).clamp(-180.0, 180.0),
    }
}

pub fn lat_lon_to_tile(lat: f32, lon: f32, zoom: u8) -> (u32, u32) {
    let lat = lat.clamp(-85.0511, 85.0511) as f64;
    let lon = lon.clamp(-180.0, 180.0) as f64;
    let zoom_scale = 2_f64.powi(i32::from(zoom));
    let x = ((lon + 180.0) / 360.0 * zoom_scale)
        .floor()
        .clamp(0.0, zoom_scale - 1.0);
    let lat_rad = lat.to_radians();
    let y = ((1.0 - ((lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI)) / 2.0
        * zoom_scale)
        .floor()
        .clamp(0.0, zoom_scale - 1.0);
    (x as u32, y as u32)
}

pub(super) fn encode_linestring_wkb(points: &[GeoPoint]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(1 + 4 + 4 + points.len() * 16);
    bytes.push(1); // little endian
    bytes.extend_from_slice(&2u32.to_le_bytes()); // LineString
    bytes.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for point in points {
        bytes.extend_from_slice(&(point.lon as f64).to_le_bytes());
        bytes.extend_from_slice(&(point.lat as f64).to_le_bytes());
    }
    bytes
}

pub(super) fn decode_linestring_wkb(bytes: &[u8]) -> Option<Vec<GeoPoint>> {
    if bytes.len() < 9 {
        return None;
    }
    if *bytes.first()? != 1 {
        return None;
    }
    let geometry_type = u32::from_le_bytes(bytes.get(1..5)?.try_into().ok()?);
    if geometry_type != 2 {
        return None;
    }
    let point_count = u32::from_le_bytes(bytes.get(5..9)?.try_into().ok()?) as usize;
    if bytes.len() < 9 + point_count * 16 {
        return None;
    }

    let mut points = Vec::with_capacity(point_count);
    let mut cursor = 9;
    for _ in 0..point_count {
        let lon = f64::from_le_bytes(bytes.get(cursor..cursor + 8)?.try_into().ok()?);
        let lat = f64::from_le_bytes(bytes.get(cursor + 8..cursor + 16)?.try_into().ok()?);
        cursor += 16;
        points.push(GeoPoint {
            lat: lat as f32,
            lon: lon as f32,
        });
    }
    Some(points)
}

#[allow(dead_code)]
pub(super) fn parse_geojson_linestring(geometry: &Value) -> Option<Vec<GeoPoint>> {
    let coordinates = geometry.get("coordinates")?.as_array()?;
    let mut points = Vec::with_capacity(coordinates.len());
    for coordinate in coordinates {
        let pair = coordinate.as_array()?;
        let lon = pair.first()?.as_f64()?;
        let lat = pair.get(1)?.as_f64()?;
        points.push(GeoPoint {
            lat: lat as f32,
            lon: lon as f32,
        });
    }
    Some(points)
}

#[allow(dead_code)]
pub(super) fn parse_geojson_multilinestring(geometry: &Value) -> Option<Vec<Vec<GeoPoint>>> {
    let coordinates = geometry.get("coordinates")?.as_array()?;
    let mut lines = Vec::with_capacity(coordinates.len());
    for line in coordinates {
        let mut points = Vec::new();
        for coordinate in line.as_array()? {
            let pair = coordinate.as_array()?;
            let lon = pair.first()?.as_f64()?;
            let lat = pair.get(1)?.as_f64()?;
            points.push(GeoPoint {
                lat: lat as f32,
                lon: lon as f32,
            });
        }
        lines.push(points);
    }
    Some(lines)
}

pub(super) fn focus_bounds(focus: GeoPoint, radius_miles: f32) -> GeoBounds {
    let radius_km = radius_miles.max(1.0) * 1.60934;
    let lat_delta = radius_km / 111.32;
    let lon_scale = (focus.lat.to_radians().cos()).abs().max(0.15);
    let lon_delta = radius_km / (111.32 * lon_scale);

    GeoBounds {
        min_lat: (focus.lat - lat_delta).clamp(-85.0511, 85.0511),
        max_lat: (focus.lat + lat_delta).clamp(-85.0511, 85.0511),
        min_lon: (focus.lon - lon_delta).clamp(-180.0, 180.0),
        max_lon: (focus.lon + lon_delta).clamp(-180.0, 180.0),
    }
}

pub(super) fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

pub(super) fn system_time_to_unix(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

pub(super) fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(super) fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub(super) fn canonical_road_class(value: &str) -> Option<&'static str> {
    match value {
        "motorway" | "motorway_link" => Some("motorway"),
        "trunk" | "trunk_link" => Some("trunk"),
        "primary" | "primary_link" => Some("primary"),
        "secondary" | "secondary_link" => Some("secondary"),
        "tertiary" | "tertiary_link" => Some("tertiary"),
        "residential" | "living_street" | "unclassified" => Some("residential"),
        "service" => Some("service"),
        _ => None,
    }
}

pub(super) fn road_class_matches(road_class: &str, layer_kind: super::RoadLayerKind) -> bool {
    match layer_kind {
        super::RoadLayerKind::Major => {
            matches!(road_class, "motorway" | "trunk" | "primary" | "secondary")
        }
        super::RoadLayerKind::Minor => matches!(road_class, "tertiary" | "residential" | "service"),
    }
}

pub(super) fn canonical_water_class(key: &str, value: &str) -> Option<(&'static str, bool)> {
    match (key, value) {
        ("waterway", "river")               => Some(("river",     false)),
        ("waterway", "stream")
        | ("waterway", "creek")             => Some(("stream",    false)),
        ("waterway", "canal")               => Some(("canal",     false)),
        ("waterway", "drain")
        | ("waterway", "ditch")             => Some(("drain",     false)),
        ("natural",  "water")               => Some(("lake",      true)),
        ("landuse",  "reservoir")
        | ("landuse",  "basin")             => Some(("reservoir", true)),
        _                                   => None,
    }
}
