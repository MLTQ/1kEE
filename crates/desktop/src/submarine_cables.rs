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
//! Parsing and rendering reuse the shared uploaded-layer pipeline in
//! `model::GeoJsonLayer`.

use crate::model::{AppModel, GeoJsonLayer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;

/// TeleGeography data, CC BY-NC-SA 3.0 — see `submarine_cables/LICENSE`.
const BUNDLED_CABLES: &str = include_str!("submarine_cables/cables.geojson");
const BUNDLED_LANDINGS: &str = include_str!("submarine_cables/landings.geojson");

pub const CABLE_LAYER_NAME: &str = "Submarine cables";
pub const LANDING_LAYER_NAME: &str = "Cable landing points";

/// Human-facing provenance for the layer drawer.
pub const PROJECT_URL: &str = "https://www.submarinecablemap.com/";
pub const ATTRIBUTION: &str = "Submarine cable data © TeleGeography (CC BY-NC-SA 3.0)";

/// Landing points use a warm accent so they read against the cables, which
/// mostly draw in their own per-cable colour from the source.
const CABLE_LAYER_COLOR: [u8; 4] = [90, 170, 255, 200];
const LANDING_LAYER_COLOR: [u8; 4] = [255, 220, 50, 210];

/// Parsed once per process, on a background thread, the first time the layer
/// is enabled. `Arc` so installing it into the model is a pointer copy.
static PARSED: OnceLock<Result<Arc<Vec<GeoJsonLayer>>, String>> = OnceLock::new();
static PARSE_STARTED: AtomicBool = AtomicBool::new(false);

/// Install the bundled layers once the operator enables them. Called every
/// frame from `app.rs`; returns immediately when the layer is off or already
/// installed, so a session that never opens it never parses anything.
pub fn tick(model: &mut AppModel) {
    if !model.show_submarine_cables || !model.submarine_cable_layers.is_empty() {
        return;
    }

    match PARSED.get() {
        Some(Ok(layers)) => {
            let cables = layers.first().map_or(0, |layer| layer.features.len());
            let landings = layers.get(1).map_or(0, |layer| layer.features.len());
            model.submarine_cable_layers = Arc::clone(layers);
            model.submarine_cable_status = format!("{cables} cables · {landings} landings");
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
                        let _ = PARSED.set(parse_bundled().map(Arc::new));
                        // Wake the UI so the layer appears without waiting for
                        // the next input event.
                        crate::app::request_repaint();
                    });
                if spawned.is_err() {
                    // No thread available: parse inline rather than never.
                    let _ = PARSED.set(parse_bundled().map(Arc::new));
                }
            }
        }
    }
}

fn parse_bundled() -> Result<Vec<GeoJsonLayer>, String> {
    Ok(vec![
        build_layer(CABLE_LAYER_NAME, BUNDLED_CABLES, CABLE_LAYER_COLOR)?,
        build_layer(LANDING_LAYER_NAME, BUNDLED_LANDINGS, LANDING_LAYER_COLOR)?,
    ])
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GeoJsonGeometry;

    #[test]
    fn bundled_snapshot_parses() {
        // Guards every regeneration of the snapshot: the app has no fallback.
        let layers = parse_bundled().expect("bundled snapshot must parse");
        let (cables, landings) = (&layers[0], &layers[1]);

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
    fn built_in_layers_start_unlabelled() {
        let layers = parse_bundled().unwrap();
        assert!(
            layers
                .iter()
                .all(|layer| !layer.show_labels && layer.visible)
        );
        assert_eq!(layers[0].color, CABLE_LAYER_COLOR);
        assert_eq!(layers[1].color, LANDING_LAYER_COLOR);
    }

    #[test]
    fn three_digit_hex_colour_expands() {
        let body = r##"{"type":"FeatureCollection","features":[{"type":"Feature",
            "properties":{"name":"Cable B","color":"#0af"},
            "geometry":{"type":"LineString","coordinates":[[2.0,2.0],[3.0,3.0]]}}]}"##;
        let layer = build_layer(CABLE_LAYER_NAME, body, CABLE_LAYER_COLOR).unwrap();
        assert_eq!(layer.features[0].color, Some([0x00, 0xaa, 0xff, 220]));
    }

    #[test]
    fn rejects_malformed_or_empty_input() {
        assert!(build_layer(CABLE_LAYER_NAME, "not json", CABLE_LAYER_COLOR).is_err());
        let empty = r#"{"type":"FeatureCollection","features":[]}"#;
        assert!(build_layer(CABLE_LAYER_NAME, empty, CABLE_LAYER_COLOR).is_err());
    }
}
