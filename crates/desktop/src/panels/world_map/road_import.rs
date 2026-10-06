//! Background road-import scheduling; no database or inventory work on the UI.
use crate::{
    model::{AppModel, GeoPoint},
    osm_ingest,
};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, mpsc};

struct Completed {
    root: Option<PathBuf>,
    messages: Vec<String>,
    inventory: Option<osm_ingest::OsmInventory>,
}

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Roads,
    Water,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Roads => "road",
            Self::Water => "water",
        }
    }
}

fn pending(kind: Kind) -> &'static Mutex<Option<mpsc::Receiver<Completed>>> {
    static ROADS: OnceLock<Mutex<Option<mpsc::Receiver<Completed>>>> = OnceLock::new();
    static WATER: OnceLock<Mutex<Option<mpsc::Receiver<Completed>>>> = OnceLock::new();
    match kind {
        Kind::Roads => &ROADS,
        Kind::Water => &WATER,
    }
    .get_or_init(|| Mutex::new(None))
}

pub(super) fn poll(model: &mut AppModel, kind: Kind) {
    let Ok(mut pending) = pending(kind).try_lock() else {
        return;
    };
    let Some(rx) = pending.as_ref() else {
        return;
    };
    match rx.try_recv() {
        Ok(result) => {
            *pending = None;
            if result.root != model.selected_root {
                return;
            }
            for message in result.messages {
                model.push_log(message);
            }
            if let Some(inventory) = result.inventory {
                model.osm_inventory = inventory;
            }
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            *pending = None;
            model.push_log(format!(
                "{} import check failed; it will retry.",
                kind.label()
            ));
        }
        Err(mpsc::TryRecvError::Empty) => {}
    }
}

pub(super) fn queue(
    model: &mut AppModel,
    kind: Kind,
    requests: Vec<(GeoPoint, f32, &'static str)>,
) {
    if requests.is_empty() {
        return;
    }
    let Ok(mut pending) = pending(kind).try_lock() else {
        return;
    };
    if pending.is_some() {
        return;
    }
    let root = model.selected_root.clone();
    let (tx, rx) = mpsc::channel();
    *pending = Some(rx);
    if let Err(error) = std::thread::Builder::new()
        .name(format!("{}-import-check", kind.label()))
        .spawn(move || {
            let mut messages = Vec::new();
            let mut changed = false;
            for (point, radius, label) in requests {
                let result = match kind {
                    Kind::Roads => {
                        osm_ingest::queue_focus_roads_import(root.as_deref(), point, radius)
                    }
                    Kind::Water => {
                        osm_ingest::queue_focus_water_import(root.as_deref(), point, radius)
                    }
                };
                match result {
                    Ok(true) => {
                        changed = true;
                        messages.push(format!(
                            "Queued focused {} import for the {label}.",
                            kind.label()
                        ));
                    }
                    Ok(false) => {}
                    Err(error) => {
                        messages.push(format!("Focused {} import failed: {error}", kind.label()))
                    }
                }
            }
            let inventory = changed.then(|| osm_ingest::OsmInventory::detect_from(root.as_deref()));
            let _ = tx.send(Completed {
                root,
                messages,
                inventory,
            });
            crate::app::request_repaint();
        })
    {
        *pending = None;
        model.push_log(format!(
            "Cannot start {} import check: {error}",
            kind.label()
        ));
    }
}
