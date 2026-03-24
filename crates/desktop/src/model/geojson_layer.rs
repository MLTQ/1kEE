use serde_json::Value;

use super::GeoPoint;

// ── Geometry types ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum GeoJsonGeometry {
    Point(GeoPoint),
    LineString(Vec<GeoPoint>),
    MultiLineString(Vec<Vec<GeoPoint>>),
    Polygon(Vec<Vec<GeoPoint>>),       // rings: outer + optional holes
    MultiPolygon(Vec<Vec<Vec<GeoPoint>>>),
}

// ── Feature ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct GeoJsonFeature {
    pub geometry: GeoJsonGeometry,
    /// Best-effort label extracted from feature properties ("name", "label", …).
    pub label: Option<String>,
}

// ── Layer ─────────────────────────────────────────────────────────────────────

/// A fully-parsed GeoJSON file loaded as a named, togglable overlay layer.
#[derive(Clone, Debug)]
pub struct GeoJsonLayer {
    pub name: String,
    pub features: Vec<GeoJsonFeature>,
    pub visible: bool,
    /// RGBA display colour auto-assigned from a palette on import.
    pub color: [u8; 4],
}

impl GeoJsonLayer {
    /// Parse raw GeoJSON text into a layer.  Returns an error description on
    /// failure; the caller should surface it in the activity log.
    pub fn parse(name: String, json: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let features = collect_features(&value)?;
        Ok(GeoJsonLayer {
            color: palette_color(&name),
            name,
            features,
            visible: true,
        })
    }
}

// ── Colour palette ────────────────────────────────────────────────────────────

fn palette_color(seed: &str) -> [u8; 4] {
    let h: u32 = seed
        .bytes()
        .fold(5381u32, |acc, b| acc.wrapping_mul(33).wrapping_add(b as u32));
    const PALETTE: &[[u8; 4]] = &[
        [255, 120,  40, 220],  // orange
        [ 80, 210, 120, 220],  // green
        [ 90, 170, 255, 220],  // blue
        [255,  80, 180, 220],  // pink
        [255, 220,  50, 220],  // yellow
        [160,  90, 255, 220],  // purple
        [ 50, 210, 210, 220],  // cyan
        [255,  70,  80, 220],  // red
    ];
    PALETTE[(h as usize) % PALETTE.len()]
}

// ── GeoJSON collection traversal ──────────────────────────────────────────────

fn collect_features(val: &Value) -> Result<Vec<GeoJsonFeature>, String> {
    match val.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => {
            let arr = val["features"]
                .as_array()
                .ok_or("FeatureCollection missing \"features\" array")?;
            let mut out = Vec::new();
            for f in arr {
                out.extend(collect_features(f).unwrap_or_default());
            }
            Ok(out)
        }
        Some("Feature") => {
            let geom = &val["geometry"];
            if geom.is_null() {
                return Ok(Vec::new());
            }
            let label = extract_label(&val["properties"]);
            // GeometryCollection inside a Feature → expand into multiple features
            if geom.get("type").and_then(Value::as_str) == Some("GeometryCollection") {
                let geoms = geom["geometries"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
                return Ok(geoms
                    .iter()
                    .filter_map(|g| parse_geometry(g).ok())
                    .map(|geometry| GeoJsonFeature {
                        geometry,
                        label: label.clone(),
                    })
                    .collect());
            }
            let geometry = parse_geometry(geom)?;
            Ok(vec![GeoJsonFeature { geometry, label }])
        }
        Some("GeometryCollection") => {
            let geoms = val["geometries"]
                .as_array()
                .ok_or("GeometryCollection missing \"geometries\"")?;
            Ok(geoms
                .iter()
                .filter_map(|g| parse_geometry(g).ok())
                .map(|geometry| GeoJsonFeature {
                    geometry,
                    label: None,
                })
                .collect())
        }
        Some(_) => {
            // Bare geometry object at top level
            match parse_geometry(val) {
                Ok(geometry) => Ok(vec![GeoJsonFeature {
                    geometry,
                    label: None,
                }]),
                Err(_) => Ok(Vec::new()),
            }
        }
        None => Err("Unrecognised GeoJSON root object (missing \"type\")".into()),
    }
}

// ── Geometry parser ───────────────────────────────────────────────────────────

fn parse_geometry(val: &Value) -> Result<GeoJsonGeometry, String> {
    match val.get("type").and_then(Value::as_str) {
        Some("Point") => Ok(GeoJsonGeometry::Point(parse_pos(&val["coordinates"])?)),
        Some("LineString") => {
            let arr = val["coordinates"]
                .as_array()
                .ok_or("LineString: bad coordinates")?;
            Ok(GeoJsonGeometry::LineString(parse_positions(arr)?))
        }
        Some("MultiLineString") => {
            let lines = val["coordinates"]
                .as_array()
                .ok_or("MultiLineString: bad coordinates")?
                .iter()
                .filter_map(|r| r.as_array())
                .filter_map(|a| parse_positions(a).ok())
                .collect();
            Ok(GeoJsonGeometry::MultiLineString(lines))
        }
        Some("Polygon") => {
            let rings: Vec<Vec<GeoPoint>> = val["coordinates"]
                .as_array()
                .ok_or("Polygon: bad coordinates")?
                .iter()
                .filter_map(|r| r.as_array())
                .filter_map(|a| parse_positions(a).ok())
                .collect();
            Ok(GeoJsonGeometry::Polygon(rings))
        }
        Some("MultiPolygon") => {
            let polys = val["coordinates"]
                .as_array()
                .ok_or("MultiPolygon: bad coordinates")?
                .iter()
                .filter_map(|p| p.as_array())
                .map(|rings| {
                    rings
                        .iter()
                        .filter_map(|r| r.as_array())
                        .filter_map(|a| parse_positions(a).ok())
                        .collect::<Vec<_>>()
                })
                .collect();
            Ok(GeoJsonGeometry::MultiPolygon(polys))
        }
        Some(t) => Err(format!("unsupported geometry type \"{t}\"")),
        None => Err("geometry missing \"type\"".into()),
    }
}

fn parse_pos(c: &Value) -> Result<GeoPoint, String> {
    let a = c.as_array().ok_or("position is not an array")?;
    if a.len() < 2 {
        return Err("position needs at least [lon, lat]".into());
    }
    Ok(GeoPoint {
        lon: a[0].as_f64().ok_or("bad longitude")? as f32,
        lat: a[1].as_f64().ok_or("bad latitude")? as f32,
    })
}

fn parse_positions(arr: &[Value]) -> Result<Vec<GeoPoint>, String> {
    arr.iter().map(parse_pos).collect()
}

// ── Label extraction ──────────────────────────────────────────────────────────

fn extract_label(props: &Value) -> Option<String> {
    if !props.is_object() {
        return None;
    }
    for key in &["name", "NAME", "Name", "label", "LABEL", "title", "TITLE"] {
        if let Some(s) = props.get(key).and_then(Value::as_str) {
            if !s.is_empty() {
                return Some(s.to_owned());
            }
        }
    }
    None
}

// ── Centroid helper (public for rendering modules) ────────────────────────────

pub fn ring_centroid(ring: &[GeoPoint]) -> Option<GeoPoint> {
    if ring.is_empty() {
        return None;
    }
    let n = ring.len() as f32;
    Some(GeoPoint {
        lat: ring.iter().map(|p| p.lat).sum::<f32>() / n,
        lon: ring.iter().map(|p| p.lon).sum::<f32>() / n,
    })
}
