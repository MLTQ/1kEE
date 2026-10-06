//! Read/decompress/parse uploaded map layers without blocking the UI.
use crate::model::{AppModel, GeoJsonLayer};
use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock, mpsc},
};
type ResultMessage = Result<(GeoJsonLayer, String), String>;
static PENDING: OnceLock<Mutex<Option<mpsc::Receiver<ResultMessage>>>> = OnceLock::new();
fn pending() -> &'static Mutex<Option<mpsc::Receiver<ResultMessage>>> {
    PENDING.get_or_init(Default::default)
}
pub(super) fn busy() -> bool {
    pending().lock().unwrap().is_some()
}
pub(super) fn queue(path: PathBuf, model: &mut AppModel) {
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
            let result = std::panic::catch_unwind(|| read(path))
                .unwrap_or_else(|_| Err("Layer import failed; please retry.".into()));
            let _ = tx.send(result);
            crate::app::request_repaint();
        })
    {
        *guard = None;
        model.push_log(format!("Cannot start layer import: {error}"));
    }
}
pub(crate) fn poll(model: &mut AppModel) {
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
        Ok(Ok((layer, format))) => {
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
fn read(path: PathBuf) -> ResultMessage {
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported layer".into());
    let extension = path.extension().and_then(|e| e.to_str());
    let format = extension.unwrap_or("LAYER").to_ascii_uppercase();
    let bytes = std::fs::read(&path).map_err(|e| format!("Layer read error: {e}"))?;
    let layer = GeoJsonLayer::parse_upload(name.clone(), extension, &bytes)
        .map_err(|e| format!("{format} parse error in \"{name}\": {e}"))?;
    Ok((layer, format))
}
