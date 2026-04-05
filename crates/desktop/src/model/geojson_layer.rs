use std::path::Path;

use serde_json::Value;

use super::GeoPoint;

// ── Geometry types ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum GeoJsonGeometry {
    Point(GeoPoint),
    LineString(Vec<GeoPoint>),
    MultiLineString(Vec<Vec<GeoPoint>>),
    Polygon(Vec<Vec<GeoPoint>>), // rings: outer + optional holes
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

/// A fully-parsed user-uploaded vector layer loaded as a named, togglable
/// overlay layer.
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

    /// Parse a supported uploaded layer file into the shared overlay model.
    pub fn parse_upload(
        name: String,
        extension: Option<&str>,
        bytes: &[u8],
    ) -> Result<Self, String> {
        let extension = extension
            .unwrap_or_default()
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase();

        match extension.as_str() {
            "geojson" | "json" => {
                let text = decode_text(bytes, "GeoJSON")?;
                Self::parse(name, &text)
            }
            "kml" => {
                let text = decode_text(bytes, "KML")?;
                Self::parse_kml(name, &text)
            }
            "kmz" => Self::parse_kmz(name, bytes),
            other if !other.is_empty() => Err(format!("unsupported layer format .{other}")),
            _ => Err("unsupported layer format".into()),
        }
    }

    /// Parse raw KML text into a layer using the same geometry model as
    /// uploaded GeoJSON.
    pub fn parse_kml(name: String, xml: &str) -> Result<Self, String> {
        let features = super::kml_layer::parse_kml_features(xml)?;
        Ok(GeoJsonLayer {
            color: palette_color(&name),
            name,
            features,
            visible: true,
        })
    }

    /// Parse a KMZ archive by loading `doc.kml` (or the first `.kml` entry)
    /// and delegating to the KML parser.
    pub fn parse_kmz(name: String, bytes: &[u8]) -> Result<Self, String> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| format!("KMZ archive error: {e}"))?;
        let mut kml_entry_name = None;
        for idx in 0..archive.len() {
            let file = archive
                .by_index(idx)
                .map_err(|e| format!("KMZ archive error: {e}"))?;
            let candidate = file.name().to_owned();
            let file_name = Path::new(&candidate)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(candidate.as_str());
            let is_doc = file_name.eq_ignore_ascii_case("doc.kml");
            let is_kml = file_name.to_ascii_lowercase().ends_with(".kml");
            drop(file);
            if is_doc {
                kml_entry_name = Some(candidate);
                break;
            }
            if is_kml && kml_entry_name.is_none() {
                kml_entry_name = Some(candidate);
            }
        }

        let entry_name = kml_entry_name.ok_or("KMZ archive does not contain a KML document")?;
        let mut file = archive
            .by_name(&entry_name)
            .map_err(|e| format!("KMZ archive error: {e}"))?;
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut file, &mut xml)
            .map_err(|e| format!("KMZ KML read error: {e}"))?;
        Self::parse_kml(name, xml.trim_start_matches('\u{feff}'))
    }
}

// ── Colour palette ────────────────────────────────────────────────────────────

fn palette_color(seed: &str) -> [u8; 4] {
    let h: u32 = seed.bytes().fold(5381u32, |acc, b| {
        acc.wrapping_mul(33).wrapping_add(b as u32)
    });
    const PALETTE: &[[u8; 4]] = &[
        [255, 120, 40, 220], // orange
        [80, 210, 120, 220], // green
        [90, 170, 255, 220], // blue
        [255, 80, 180, 220], // pink
        [255, 220, 50, 220], // yellow
        [160, 90, 255, 220], // purple
        [50, 210, 210, 220], // cyan
        [255, 70, 80, 220],  // red
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
                let geoms = geom["geometries"]
                    .as_array()
                    .map(|a| a.as_slice())
                    .unwrap_or(&[]);
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

fn decode_text(bytes: &[u8], label: &str) -> Result<String, String> {
    String::from_utf8(bytes.to_vec())
        .map(|text| text.trim_start_matches('\u{feff}').to_owned())
        .map_err(|e| format!("{label} text must be UTF-8: {e}"))
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
