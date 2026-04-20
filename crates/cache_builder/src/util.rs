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

/// Parse an OSM voltage string (raw volts like "110000", or kV like "110 kV") to kV integer.
pub fn parse_voltage_kv(value: &str) -> Option<i32> {
    // Strip whitespace and take the leading numeric run
    let trimmed = value.trim();
    let numeric: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    let raw: i32 = numeric.parse().ok()?;
    // If the value mentions kV or kv, treat raw as already-kV
    if trimmed.to_ascii_lowercase().contains("kv") {
        Some(raw)
    } else {
        // OSM stores volts (e.g. 110000 → 110 kV); protect against < 1000 values
        if raw >= 1000 { Some(raw / 1000) } else { Some(raw) }
    }
}

/// Returns the canonical POWR class name (matches `decode_powr_class`) for
/// a power= tag value plus an optional voltage in kV.
pub fn canonical_power_class(power_type: &str, voltage_kv: Option<i32>) -> Option<&'static str> {
    match power_type {
        "line" | "cable" => Some(match voltage_kv {
            Some(v) if v >= 300 => "line_ultra",
            Some(v) if v >= 100 => "line_high",
            Some(v) if v >= 50  => "line_med",
            Some(v) if v > 0    => "line_low",
            _                   => "minor_line",
        }),
        "minor_line" => Some("minor_line"),
        "substation" | "sub_station" => Some("substation"),
        "plant" | "generator" => Some("power_plant"),
        "tower" | "pole" => Some("tower"),
        _ => None,
    }
}

pub fn canonical_railway_class(value: &str) -> Option<&'static str> {
    match value {
        "mainline" => Some("mainline"),
        "rail" => Some("rail"),
        "subway" => Some("subway"),
        "tram" => Some("tram"),
        "light_rail" => Some("light_rail"),
        "narrow_gauge" => Some("narrow_gauge"),
        "funicular" | "cable_car" => Some("funicular"),
        "monorail" => Some("monorail"),
        "disused" | "abandoned" | "razed" => Some("disused"),
        _ => None,
    }
}

/// Returns the canonical PIPE class from a `substance=` tag (or "" for unknown).
pub fn canonical_pipeline_class(substance: &str) -> &'static str {
    match substance {
        "gas" | "natural_gas" | "lpg" | "methane" => "gas",
        "oil" | "fuel" | "petroleum" | "kerosene" | "diesel" | "crude_oil" => "oil",
        "water" | "rainwater" | "drinking_water" | "hot_water" => "water",
        "sewage" | "wastewater" | "sewer" | "waste_water" => "sewer",
        _ => "other",
    }
}

/// Returns the canonical AERO class from an `aeroway=` tag value.
pub fn canonical_aeroway_class(value: &str, is_international: bool) -> Option<&'static str> {
    match value {
        "aerodrome" => Some(if is_international { "intl_airport" } else { "dom_airport" }),
        "runway" => Some("runway"),
        "helipad" => Some("helipad"),
        "terminal" => Some("terminal"),
        "airstrip" => Some("airstrip"),
        _ => None,
    }
}

/// Returns the canonical MILT class from a `military=` or `landuse=military` tag.
pub fn canonical_military_class(key: &str, value: &str) -> Option<&'static str> {
    match (key, value) {
        ("military", "base") | ("military", "installation") | ("landuse", "military") => {
            Some("base")
        }
        ("military", "danger_area") | ("military", "range") | ("military", "training_area") => {
            Some("danger_area")
        }
        ("military", "airbase") | ("military", "air_base") => Some("airbase"),
        ("military", "naval_base") => Some("naval_base"),
        ("military", "barracks") => Some("barracks"),
        ("military", "checkpoint") => Some("checkpoint"),
        ("military", _) => Some("base"),
        _ => None,
    }
}

/// Returns the canonical COMM class from `man_made=` and optional `tower:type=` values.
pub fn canonical_comm_class(man_made: &str, tower_type: Option<&str>) -> Option<&'static str> {
    match man_made {
        "tower" | "mast" => match tower_type {
            Some("communication" | "radio" | "television" | "broadcast") => Some("comm_tower"),
            Some("radar") => Some("radar"),
            _ => None, // generic towers are not communication infra
        },
        "communications_tower" | "communication_tower" => Some("comm_tower"),
        "telephone_exchange" => Some("telephone_exchange"),
        "data_center" | "data_centre" => Some("data_center"),
        "satellite_dish" => Some("satellite_dish"),
        _ => None,
    }
}

/// Returns the canonical INDS class from landuse/man_made tags.
pub fn canonical_industrial_class(key: &str, value: &str) -> Option<&'static str> {
    match (key, value) {
        ("landuse", "industrial") => Some("industrial"),
        ("landuse", "quarry") | ("landuse", "landfill") => Some("mine"),
        ("man_made", "works") | ("man_made", "factory") => Some("factory"),
        ("man_made", "power_station") | ("man_made", "nuclear_reactor") => Some("power_plant"),
        ("man_made", "mineshaft") => Some("mine"),
        ("man_made", "storage_tank") | ("man_made", "silo") => Some("storage"),
        ("man_made", "petroleum_well") | ("man_made", "oil_well") => Some("oil_terminal"),
        _ => None,
    }
}

/// Returns the canonical PORT class from various maritime tags.
pub fn canonical_port_class(key: &str, value: &str) -> Option<&'static str> {
    match (key, value) {
        ("harbour", v) if v != "no" && !v.is_empty() => Some("harbour"),
        ("leisure", "marina") => Some("marina"),
        ("amenity", "ferry_terminal") => Some("ferry_terminal"),
        ("waterway", "shipyard") | ("industrial", "shipyard") => Some("shipyard"),
        ("man_made", "pier") | ("man_made", "breakwater") | ("man_made", "quay") => {
            Some("harbour")
        }
        ("man_made", "lighthouse") | ("landmark", "lighthouse") => Some("lighthouse"),
        _ => None,
    }
}

/// Returns the canonical GOVT class from amenity/office/government tags.
pub fn canonical_govt_class(key: &str, value: &str) -> Option<&'static str> {
    match (key, value) {
        ("amenity", "police") | ("amenity", "police_station") => Some("police"),
        ("amenity", "fire_station") => Some("fire_station"),
        ("amenity", "prison") | ("amenity", "correctional_facility") => Some("prison"),
        ("amenity", "courthouse") => Some("courthouse"),
        ("amenity", "embassy") | ("diplomatic", "embassy") => Some("embassy"),
        ("amenity", "customs") | ("border_control", _) => Some("customs"),
        ("barrier", "border_control") => Some("border_crossing"),
        ("office", "government") | ("government", _) => Some("government"),
        _ => None,
    }
}

/// Returns the canonical SURV class from man_made/surveillance tags.
pub fn canonical_surv_class(key: &str, value: &str) -> Option<&'static str> {
    match (key, value) {
        ("man_made", "surveillance") => Some("cctv"),
        ("surveillance", "camera") | ("surveillance", "cctv") | ("surveillance", "CCTV") => {
            Some("cctv")
        }
        ("surveillance", "radar") | ("surveillance", "fixed") => Some("surveillance_station"),
        ("highway", "speed_camera") => Some("speed_camera"),
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
