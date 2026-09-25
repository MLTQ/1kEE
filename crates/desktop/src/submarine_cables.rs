//! Built-in submarine cable layer from a bundled TeleGeography snapshot.
//!
//! Submarine cables and their landing points are the physical substrate of the
//! internet, which makes them standing context for almost any event on the
//! globe. They also change only a handful of times a year, so rather than
//! fetching and caching them at runtime the app compiles in a snapshot
//! (`submarine_cables/*.geojson`) and parses it the first time the layer is
//! switched on. There is no network access here at all. Refresh the snapshot
//! with `tools/fetch_submarine_cables.py`.
//!
//! Rendering reuses the shared uploaded-layer pipeline in `model::GeoJsonLayer`.
//! The detail behind the landing-point tooltip and panel — country, and each
//! cable's owners, length and ready-for-service date — lives in a
//! [`CableCatalog`] parsed alongside the layers.

use crate::model::{AppModel, GeoJsonFeature, GeoJsonGeometry, GeoJsonLayer, GeoPoint};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;

/// TeleGeography data, CC BY-NC-SA 3.0 — see `submarine_cables/LICENSE`.
const BUNDLED_CABLES: &str = include_str!("submarine_cables/cables.geojson");
const BUNDLED_LANDINGS: &str = include_str!("submarine_cables/landings.geojson");

pub const CABLE_LAYER_NAME: &str = "Submarine cables";
pub const LANDING_LAYER_NAME: &str = "Cable landing points";

/// Position of each layer in `AppModel::submarine_cable_layers`.
pub const CABLE_LAYER_INDEX: usize = 0;
pub const LANDING_LAYER_INDEX: usize = 1;

/// Human-facing provenance for the layer drawer.
pub const PROJECT_URL: &str = "https://www.submarinecablemap.com/";
pub const ATTRIBUTION: &str = "Submarine cable data © TeleGeography (CC BY-NC-SA 3.0)";

/// Landing points use a warm accent so they read against the cables, which
/// mostly draw in their own per-cable colour from the source.
const CABLE_LAYER_COLOR: [u8; 4] = [90, 170, 255, 200];
pub const LANDING_LAYER_COLOR: [u8; 4] = [255, 220, 50, 210];

/// One cable system. Several route features can share a cable (one system
/// drawn as separate segments); the catalogue holds each cable once.
#[derive(Clone, Debug, PartialEq)]
pub struct CableInfo {
    pub name: String,
    pub color: [u8; 4],
    pub length: Option<String>,
    pub owners: Option<String>,
    /// Ready-for-service date as published, e.g. "2014 December".
    pub rfs: Option<String>,
    pub rfs_year: Option<u16>,
    pub is_planned: bool,
    pub url: Option<String>,
}

/// A coastal station where one or more cables come ashore.
#[derive(Clone, Debug, PartialEq)]
pub struct LandingPoint {
    pub name: String,
    pub country: Option<String>,
    pub location: GeoPoint,
    /// Indices into [`CableCatalog::cables`], sorted by cable name.
    pub cables: Vec<usize>,
}

/// Detail behind the landing-point tooltip and panel.
///
/// `landings[i]` is the same station as feature `i` of the landing layer: the
/// layer is built from this list, so an index hit-tested on the map always
/// resolves to the right record.
#[derive(Debug, Default)]
pub struct CableCatalog {
    pub cables: Vec<CableInfo>,
    pub landings: Vec<LandingPoint>,
}

struct Bundle {
    layers: Arc<Vec<GeoJsonLayer>>,
    catalog: CableCatalog,
}

/// Parsed once per process, on a background thread, the first time the layer
/// is enabled. Layers are `Arc` so installing them into the model is a pointer
/// copy.
static PARSED: OnceLock<Result<Bundle, String>> = OnceLock::new();
static PARSE_STARTED: AtomicBool = AtomicBool::new(false);

/// The landing-point catalogue, once the snapshot has been parsed.
pub fn catalog() -> Option<&'static CableCatalog> {
    PARSED.get()?.as_ref().ok().map(|bundle| &bundle.catalog)
}

/// Whether landing points are on screen: the cable layer is enabled, parsed,
/// and the operator has not hidden the landing layer. Map hit-testing and
/// tooltips key off this so hidden dots can never be hovered or clicked.
pub fn landing_points_visible(model: &AppModel) -> bool {
    model.show_submarine_cables
        && model.active_body == crate::model::ActiveBody::Earth
        && model
            .submarine_cable_layers
            .get(LANDING_LAYER_INDEX)
            .is_some_and(|layer| layer.visible)
}

/// Install the bundled layers once the operator enables them. Called every
/// frame from `app.rs`; returns immediately when the layer is off or already
/// installed, so a session that never opens it never parses anything.
pub fn tick(model: &mut AppModel) {
    if !model.show_submarine_cables || !model.submarine_cable_layers.is_empty() {
        return;
    }

    match PARSED.get() {
        Some(Ok(bundle)) => {
            model.submarine_cable_layers = Arc::clone(&bundle.layers);
            model.submarine_cable_status = format!(
                "{} cables · {} landings",
                bundle.catalog.cables.len(),
                bundle.catalog.landings.len()
            );
        }
        Some(Err(error)) => {
            // Guarded by `bundled_snapshot_parses`, so this means a bad
            // regeneration slipped through. Only write the status once.
            if !model
                .submarine_cable_status
                .starts_with("bundled data invalid")
            {
                model.submarine_cable_status = format!("bundled data invalid: {error}");
                model.push_log(format!("Submarine cables: {error}"));
            }
        }
        None => {
            if !PARSE_STARTED.swap(true, Ordering::SeqCst) {
                model.submarine_cable_status = "loading…".into();
                let spawned = thread::Builder::new()
                    .name("submarine-cables".into())
                    .spawn(|| {
                        let _ = PARSED.set(parse_bundled());
                        // Wake the UI so the layer appears without waiting for
                        // the next input event.
                        crate::app::request_repaint();
                    });
                if spawned.is_err() {
                    // No thread available: parse inline rather than never.
                    let _ = PARSED.set(parse_bundled());
                }
            }
        }
    }
}

fn parse_bundled() -> Result<Bundle, String> {
    let cable_layer = build_layer(CABLE_LAYER_NAME, BUNDLED_CABLES, CABLE_LAYER_COLOR)?;
    let catalog = parse_catalog(BUNDLED_CABLES, BUNDLED_LANDINGS)?;
    let landing_layer = landing_layer(&catalog);
    Ok(Bundle {
        layers: Arc::new(vec![cable_layer, landing_layer]),
        catalog,
    })
}

/// Parse one GeoJSON payload into a built-in overlay layer.
///
/// Built-in layers start unlabelled: the globe view has no viewport culling
/// for labels, and ~2,600 cable and landing names drawn at once is unreadable.
/// The names stay on the features for the layer drawer's label toggle.
fn build_layer(name: &str, body: &str, color: [u8; 4]) -> Result<GeoJsonLayer, String> {
    let mut layer =
        GeoJsonLayer::parse(name.to_owned(), body).map_err(|error| format!("{name}: {error}"))?;
    if layer.features.is_empty() {
        return Err(format!("{name}: no features"));
    }
    layer.color = color;
    layer.visible = true;
    layer.show_labels = false;
    Ok(layer)
}

/// Build the landing layer straight from the catalogue so feature `i` and
/// `catalog.landings[i]` are the same station by construction.
fn landing_layer(catalog: &CableCatalog) -> GeoJsonLayer {
    GeoJsonLayer {
        name: LANDING_LAYER_NAME.to_owned(),
        features: catalog
            .landings
            .iter()
            .map(|landing| GeoJsonFeature {
                geometry: GeoJsonGeometry::Point(landing.location),
                label: Some(landing.name.clone()),
                color: None,
            })
            .collect(),
        visible: true,
        color: LANDING_LAYER_COLOR,
        show_labels: false,
    }
}

fn parse_catalog(cables_body: &str, landings_body: &str) -> Result<CableCatalog, String> {
    let mut catalog = CableCatalog::default();
    let mut index_by_id = HashMap::new();

    for feature in features(cables_body, CABLE_LAYER_NAME)? {
        let props = &feature["properties"];
        let (Some(id), Some(name)) = (text(props, "id"), text(props, "name")) else {
            continue;
        };
        // A cable drawn as several route segments appears once per segment.
        if index_by_id.contains_key(&id) {
            continue;
        }
        index_by_id.insert(id, catalog.cables.len());
        catalog.cables.push(CableInfo {
            color: text(props, "color")
                .and_then(|hex| crate::model::parse_hex_color(&hex))
                .unwrap_or(CABLE_LAYER_COLOR),
            name,
            length: text(props, "length"),
            owners: text(props, "owners"),
            rfs: text(props, "rfs"),
            rfs_year: props
                .get("rfs_year")
                .and_then(Value::as_u64)
                .and_then(|year| u16::try_from(year).ok()),
            is_planned: props.get("is_planned").and_then(Value::as_bool) == Some(true),
            url: text(props, "url"),
        });
    }

    for feature in features(landings_body, LANDING_LAYER_NAME)? {
        let props = &feature["properties"];
        let Some(name) = text(props, "name") else {
            continue;
        };
        let coordinates = &feature["geometry"]["coordinates"];
        let (Some(lon), Some(lat)) = (coordinates[0].as_f64(), coordinates[1].as_f64()) else {
            continue;
        };
        let mut cables: Vec<usize> = props["cables"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .filter_map(|id| index_by_id.get(id).copied())
                    .collect()
            })
            .unwrap_or_default();
        cables.sort_by(|a, b| catalog.cables[*a].name.cmp(&catalog.cables[*b].name));
        cables.dedup();

        catalog.landings.push(LandingPoint {
            name,
            country: text(props, "country"),
            location: GeoPoint {
                lat: lat as f32,
                lon: lon as f32,
            },
            cables,
        });
    }

    if catalog.cables.is_empty() || catalog.landings.is_empty() {
        return Err("cable catalogue is empty".into());
    }
    Ok(catalog)
}

fn features(body: &str, source: &str) -> Result<Vec<Value>, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| format!("{source}: {error}"))?;
    match value {
        Value::Object(mut root) => match root.remove("features") {
            Some(Value::Array(features)) => Ok(features),
            _ => Err(format!("{source}: no features array")),
        },
        _ => Err(format!("{source}: not a GeoJSON object")),
    }
}

fn text(props: &Value, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_snapshot_parses() {
        // Guards every regeneration of the snapshot: the app has no fallback.
        let bundle = parse_bundled().expect("bundled snapshot must parse");
        let cables = &bundle.layers[CABLE_LAYER_INDEX];
        let landings = &bundle.layers[LANDING_LAYER_INDEX];

        assert!(
            cables.features.len() > 500,
            "{} cables",
            cables.features.len()
        );
        assert!(
            landings.features.len() > 1000,
            "{} landings",
            landings.features.len()
        );

        // Every cable carries its own name and published colour.
        assert!(cables.features.iter().all(|f| f.label.is_some()));
        assert!(cables.features.iter().all(|f| f.color.is_some()));
        assert!(cables.features.iter().all(|f| matches!(
            f.geometry,
            GeoJsonGeometry::MultiLineString(_) | GeoJsonGeometry::LineString(_)
        )));
        assert!(
            landings
                .features
                .iter()
                .all(|f| matches!(f.geometry, GeoJsonGeometry::Point(_)))
        );
    }

    #[test]
    fn landing_layer_and_catalogue_are_index_aligned() {
        // Map clicks resolve a landing by its feature index, so the two lists
        // must describe the same station at every position.
        let bundle = parse_bundled().unwrap();
        let layer = &bundle.layers[LANDING_LAYER_INDEX];
        assert_eq!(layer.features.len(), bundle.catalog.landings.len());
        for (feature, landing) in layer.features.iter().zip(&bundle.catalog.landings) {
            assert_eq!(feature.label.as_deref(), Some(landing.name.as_str()));
            assert!(matches!(feature.geometry, GeoJsonGeometry::Point(p) if p == landing.location));
        }
    }

    #[test]
    fn bundled_landings_carry_country_and_cables() {
        let catalog = parse_bundled().unwrap().catalog;
        let with_cables = catalog
            .landings
            .iter()
            .filter(|l| !l.cables.is_empty())
            .count();
        let with_country = catalog
            .landings
            .iter()
            .filter(|l| l.country.is_some())
            .count();
        // Allow a little slack for stations TeleGeography lists before a cable.
        assert!(
            with_cables * 100 >= catalog.landings.len() * 95,
            "{with_cables} linked"
        );
        assert!(
            with_country * 100 >= catalog.landings.len() * 95,
            "{with_country} with country"
        );
        // Cable indices are valid and sorted by name.
        for landing in &catalog.landings {
            assert!(landing.cables.iter().all(|&i| i < catalog.cables.len()));
            assert!(
                landing
                    .cables
                    .windows(2)
                    .all(|pair| { catalog.cables[pair[0]].name <= catalog.cables[pair[1]].name })
            );
        }
    }

    #[test]
    fn catalogue_dedupes_multi_segment_cables_and_reads_details() {
        let cables = r##"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"id":"a","name":"Alpha","color":"#0af",
              "length":"402 km","owners":"BT","rfs":"2014 December","rfs_year":2014},
             "geometry":{"type":"LineString","coordinates":[[0,0],[1,1]]}},
            {"type":"Feature","properties":{"id":"a","name":"Alpha","color":"#0af"},
             "geometry":{"type":"LineString","coordinates":[[1,1],[2,2]]}},
            {"type":"Feature","properties":{"id":"b","name":"Beta","is_planned":true,"rfs_year":2029},
             "geometry":{"type":"LineString","coordinates":[[3,3],[4,4]]}}]}"##;
        let landings = r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"name":"Port, Land","country":"Land","cables":["b","a","missing"]},
             "geometry":{"type":"Point","coordinates":[10.5,20.25]}}]}"#;

        let catalog = parse_catalog(cables, landings).unwrap();
        assert_eq!(
            catalog.cables.len(),
            2,
            "segments of one cable collapse to one entry"
        );

        let alpha = &catalog.cables[0];
        assert_eq!(alpha.color, [0x00, 0xaa, 0xff, 220]);
        assert_eq!(alpha.length.as_deref(), Some("402 km"));
        assert_eq!(alpha.owners.as_deref(), Some("BT"));
        assert_eq!(alpha.rfs_year, Some(2014));
        assert!(!alpha.is_planned);
        assert!(catalog.cables[1].is_planned);

        let landing = &catalog.landings[0];
        assert_eq!(landing.country.as_deref(), Some("Land"));
        assert_eq!(
            landing.location,
            GeoPoint {
                lat: 20.25,
                lon: 10.5
            }
        );
        // Unknown ids are dropped; the rest are sorted by cable name.
        let names: Vec<_> = landing
            .cables
            .iter()
            .map(|&i| catalog.cables[i].name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "Beta"]);
    }

    #[test]
    fn rejects_malformed_or_empty_input() {
        assert!(build_layer(CABLE_LAYER_NAME, "not json", CABLE_LAYER_COLOR).is_err());
        let empty = r#"{"type":"FeatureCollection","features":[]}"#;
        assert!(build_layer(CABLE_LAYER_NAME, empty, CABLE_LAYER_COLOR).is_err());
        assert!(parse_catalog(empty, empty).is_err());
    }
}
