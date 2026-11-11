use roxmltree::{Document, Node};

use super::{GeoJsonFeature, GeoJsonGeometry, GeoPoint};

pub(super) fn parse_kml_features(xml: &str) -> Result<Vec<GeoJsonFeature>, String> {
    let document = Document::parse(xml).map_err(|e| format!("KML XML parse error: {e}"))?;
    let mut features = Vec::new();

    for placemark in placemarks(&document) {
        features.extend(placemark_features(placemark));
    }

    if features.is_empty() {
        return Err(
            "KML did not contain any supported Point, LineString, or Polygon placemarks".into(),
        );
    }

    Ok(features)
}

fn placemarks<'a>(document: &'a Document<'_>) -> impl Iterator<Item = Node<'a, 'a>> {
    document
        .descendants()
        .filter(|node| node.has_tag_name("Placemark"))
}

fn placemark_features(placemark: Node<'_, '_>) -> Vec<GeoJsonFeature> {
    let label = direct_child_text(placemark, "name");
    parse_placemark_geometries(placemark)
        .unwrap_or_default()
        .into_iter()
        .map(|geometry| GeoJsonFeature {
            geometry,
            label: label.clone(),
            color: None,
        })
        .collect()
}

/// ArcGIS exports use KML folders for independently displayable map layers.
pub(super) fn folder_layers(
    xml: &str,
    export: &str,
    mut on_placemark: impl FnMut(&str, Node<'_, '_>, &[GeoJsonFeature]),
) -> Result<Vec<super::GeoJsonLayer>, String> {
    use std::collections::BTreeMap;
    let document = Document::parse(xml).map_err(|e| format!("KML XML parse error: {e}"))?;
    if !document.root_element().has_tag_name("kml") {
        return Err("Expected a KML document".into());
    }
    let mut folders = BTreeMap::new();
    let mut has_placemarks = false;
    for placemark in placemarks(&document) {
        has_placemarks = true;
        let features = placemark_features(placemark);
        if features.is_empty() {
            continue;
        }
        let mut names: Vec<_> = placemark
            .ancestors()
            .filter(|n| n.has_tag_name("Folder"))
            .filter_map(|n| direct_child_text(n, "name"))
            .collect();
        names.reverse();
        let name = if names.is_empty() {
            export.to_owned()
        } else {
            names.join(" / ")
        };
        on_placemark(&name, placemark, &features);
        let layer = folders
            .entry(name.clone())
            .or_insert_with(|| super::GeoJsonLayer {
                color: super::geojson_layer::palette_color(&name),
                name,
                features: Vec::new(),
                visible: true,
                show_labels: false,
            });
        layer.features.extend(features);
    }
    if folders.is_empty()
        && (has_placemarks
            || document
                .descendants()
                .any(|n| n.has_tag_name("NetworkLink") || n.has_tag_name("GroundOverlay")))
    {
        return Err("KMZ has no supported placemark geometry".into());
    }
    Ok(folders.into_values().collect())
}

fn parse_placemark_geometries(node: Node<'_, '_>) -> Result<Vec<GeoJsonGeometry>, String> {
    let mut geometries = Vec::new();
    for child in node.children().filter(|child| child.is_element()) {
        geometries.extend(parse_geometry_element(child)?);
    }
    if geometries.is_empty() {
        return Err("Placemark does not contain a supported geometry".into());
    }
    Ok(geometries)
}

fn parse_geometry_element(node: Node<'_, '_>) -> Result<Vec<GeoJsonGeometry>, String> {
    match node.tag_name().name() {
        "Point" => Ok(vec![GeoJsonGeometry::Point(parse_point(node)?)]),
        "LineString" => Ok(vec![GeoJsonGeometry::LineString(parse_coordinates_node(
            node,
        )?)]),
        "Polygon" => Ok(vec![GeoJsonGeometry::Polygon(parse_polygon(node)?)]),
        "MultiGeometry" => {
            let mut geometries = Vec::new();
            for child in node.children().filter(|child| child.is_element()) {
                geometries.extend(parse_geometry_element(child)?);
            }
            Ok(geometries)
        }
        _ => Ok(Vec::new()),
    }
}

fn parse_point(node: Node<'_, '_>) -> Result<GeoPoint, String> {
    let mut points = parse_coordinates_node(node)?;
    points
        .drain(..)
        .next()
        .ok_or("Point missing coordinates".into())
}

fn parse_polygon(node: Node<'_, '_>) -> Result<Vec<Vec<GeoPoint>>, String> {
    let mut rings = Vec::new();
    for boundary_name in ["outerBoundaryIs", "innerBoundaryIs"] {
        for boundary in node
            .children()
            .filter(|child| child.is_element() && child.tag_name().name() == boundary_name)
        {
            let linear_ring = boundary
                .children()
                .find(|child| child.is_element() && child.tag_name().name() == "LinearRing")
                .ok_or("Polygon boundary missing LinearRing")?;
            rings.push(parse_coordinates_node(linear_ring)?);
        }
    }
    if rings.is_empty() {
        return Err("Polygon missing boundary rings".into());
    }
    Ok(rings)
}

fn parse_coordinates_node(node: Node<'_, '_>) -> Result<Vec<GeoPoint>, String> {
    let coordinates = node
        .descendants()
        .find(|child| child.is_element() && child.tag_name().name() == "coordinates")
        .and_then(|node| node.text())
        .ok_or("geometry missing coordinates")?;
    parse_coordinates(coordinates)
}

fn parse_coordinates(text: &str) -> Result<Vec<GeoPoint>, String> {
    let mut points = Vec::new();
    for token in text.split_whitespace() {
        let mut parts = token.split(',');
        let lon = parts
            .next()
            .ok_or("coordinate missing longitude")?
            .trim()
            .parse::<f32>()
            .map_err(|e| format!("invalid longitude: {e}"))?;
        let lat = parts
            .next()
            .ok_or("coordinate missing latitude")?
            .trim()
            .parse::<f32>()
            .map_err(|e| format!("invalid latitude: {e}"))?;
        points.push(GeoPoint { lat, lon });
    }
    if points.is_empty() {
        return Err("coordinates list is empty".into());
    }
    Ok(points)
}

fn direct_child_text(node: Node<'_, '_>, child_name: &str) -> Option<String> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == child_name)
        .and_then(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::parse_kml_features;
    use crate::model::{GeoJsonGeometry, GeoJsonLayer};

    #[test]
    fn parses_kml_points_and_polygon() {
        let xml = r#"
            <kml xmlns="http://www.opengis.net/kml/2.2">
              <Document>
                <Placemark>
                  <name>Zone</name>
                  <Polygon>
                    <outerBoundaryIs>
                      <LinearRing>
                        <coordinates>
                          -77.0,38.8 -77.1,38.8 -77.1,38.9 -77.0,38.8
                        </coordinates>
                      </LinearRing>
                    </outerBoundaryIs>
                  </Polygon>
                </Placemark>
                <Placemark>
                  <name>Marker</name>
                  <Point><coordinates>-76.9,38.85,0</coordinates></Point>
                </Placemark>
              </Document>
            </kml>
        "#;

        let features = parse_kml_features(xml).expect("valid kml");
        assert_eq!(features.len(), 2);
        assert!(matches!(features[0].geometry, GeoJsonGeometry::Polygon(_)));
        assert!(matches!(features[1].geometry, GeoJsonGeometry::Point(_)));
        assert_eq!(features[1].label.as_deref(), Some("Marker"));
    }

    #[test]
    fn expands_multigeometry_linestrings() {
        let xml = r#"
            <kml xmlns="http://www.opengis.net/kml/2.2">
              <Document>
                <Placemark>
                  <name>River</name>
                  <MultiGeometry>
                    <LineString><coordinates>-1,51 -2,52</coordinates></LineString>
                    <LineString><coordinates>-3,53 -4,54</coordinates></LineString>
                  </MultiGeometry>
                </Placemark>
              </Document>
            </kml>
        "#;

        let features = parse_kml_features(xml).expect("valid multigeometry");
        assert_eq!(features.len(), 2);
        assert!(
            features
                .iter()
                .all(|feature| matches!(feature.geometry, GeoJsonGeometry::LineString(_)))
        );
    }

    #[test]
    fn parses_kmz_doc_kml() {
        let xml = r#"
            <kml xmlns="http://www.opengis.net/kml/2.2">
              <Document>
                <Placemark>
                  <name>Imported</name>
                  <Point><coordinates>-4.2,57.48,0</coordinates></Point>
                </Placemark>
              </Document>
            </kml>
        "#;

        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            writer
                .start_file("doc.kml", SimpleFileOptions::default())
                .expect("start file");
            writer.write_all(xml.as_bytes()).expect("write kml");
            writer.finish().expect("finish kmz");
        }

        let layer =
            GeoJsonLayer::parse_kmz("GhostMaps".into(), cursor.get_ref()).expect("valid kmz");
        assert_eq!(layer.features.len(), 1);
        assert_eq!(layer.features[0].label.as_deref(), Some("Imported"));
    }
}
