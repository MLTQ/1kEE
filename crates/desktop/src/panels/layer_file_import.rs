//! Read/decompress/parse uploaded map layers without blocking the UI.
use super::world_map::satellite_layer::SatelliteLayer;
use crate::model::{AppModel, GeoJsonLayer};
use crate::satellite_image;
use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock, mpsc},
};
enum ImportedLayer {
    Vector(GeoJsonLayer, String),
    Satellite(String, satellite_image::SatelliteImage),
}
type ResultMessage = Result<ImportedLayer, String>;
static PENDING: OnceLock<Mutex<Option<mpsc::Receiver<ResultMessage>>>> = OnceLock::new();
fn pending() -> &'static Mutex<Option<mpsc::Receiver<ResultMessage>>> {
    PENDING.get_or_init(Default::default)
}
pub(crate) fn busy() -> bool {
    pending().lock().unwrap().is_some()
}
pub(crate) fn queue(path: PathBuf, model: &mut AppModel, max_texture_side: usize) {
    let mut guard = pending().lock().unwrap();
    if guard.is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    *guard = Some(rx);
    model.push_log("Loading imported map layer…".into());
    if let Err(error) = std::thread::Builder::new()
        .name("map-layer-import".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(|| read(path, max_texture_side))
                .unwrap_or_else(|_| Err("Layer import failed; please retry.".into()));
            let _ = tx.send(result);
            crate::app::request_repaint();
        })
    {
        *guard = None;
        model.push_log(format!("Cannot start layer import: {error}"));
    }
}
pub(crate) fn poll(ctx: &egui::Context, model: &mut AppModel) {
    let mut guard = pending().lock().unwrap();
    let Some(rx) = guard.as_ref() else {
        return;
    };
    match rx.try_recv() {
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => {
            model.push_log("Layer import failed; please retry.".into())
        }
        Ok(Err(error)) => model.push_log(error),
        Ok(Ok(ImportedLayer::Satellite(name, image))) => {
            model.push_log(format!("GeoTIFF \"{name}\" loaded on Earth."));
            model
                .satellite_layers
                .push(SatelliteLayer::new(ctx, name, image));
        }
        Ok(Ok(ImportedLayer::Vector(layer, format))) => {
            model.push_log(format!(
                "{format} layer \"{}\" loaded — {} feature(s).",
                layer.name,
                layer.features.len()
            ));
            model.geojson_layers.push(layer);
        }
    }
    *guard = None;
}
fn read(path: PathBuf, max_texture_side: usize) -> ResultMessage {
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported layer".into());
    let extension = path.extension().and_then(|e| e.to_str());
    let format = extension.unwrap_or("LAYER").to_ascii_uppercase();
    if matches!(format.as_str(), "TIF" | "TIFF") {
        let image = satellite_image::read(&path, max_texture_side)
            .map_err(|e| format!("GeoTIFF import failed for \"{name}\": {e}"))?;
        return Ok(ImportedLayer::Satellite(name, image));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("Layer read error: {e}"))?;
    let layer = GeoJsonLayer::parse_upload(name.clone(), extension, &bytes)
        .map_err(|e| format!("{format} parse error in \"{name}\": {e}"))?;
    Ok(ImportedLayer::Vector(layer, format))
}

/// Explicit file-picker action, displayed in the Layers sidebar.
pub(crate) fn button(ui: &mut egui::Ui, model: &mut AppModel) {
    let importing = busy();
    if ui
        .add_enabled(
            !importing,
            egui::Button::new(if importing {
                "Loading layer…"
            } else {
                "Import Layer…"
            }),
        )
        .on_hover_text("Add vector layers or a projected satellite GeoTIFF")
        .clicked()
    {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Map layers",
                &["geojson", "json", "kml", "kmz", "tif", "tiff"],
            )
            .add_filter("Satellite GeoTIFF", &["tif", "tiff"])
            .add_filter("GeoJSON", &["geojson", "json"])
            .add_filter("KML / KMZ", &["kml", "kmz"])
            .set_title("Import map layer")
            .pick_file()
        else {
            return;
        };
        queue(path, model, ui.ctx().input(|i| i.max_texture_side));
    }
}
