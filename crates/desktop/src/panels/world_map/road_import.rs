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

fn pending() -> &'static Mutex<Option<mpsc::Receiver<Completed>>> {
    static PENDING: OnceLock<Mutex<Option<mpsc::Receiver<Completed>>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}

pub(super) fn poll(model: &mut AppModel) {
    let Ok(mut pending) = pending().try_lock() else {
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
            model.push_log("Road import check failed; it will retry.".into());
        }
        Err(mpsc::TryRecvError::Empty) => {}
    }
}

pub(super) fn queue(model: &mut AppModel, requests: Vec<(GeoPoint, f32, &'static str)>) {
    if requests.is_empty() {
        return;
    }
    let Ok(mut pending) = pending().try_lock() else {
        return;
    };
    if pending.is_some() {
        return;
    }
    let root = model.selected_root.clone();
    let (tx, rx) = mpsc::channel();
    *pending = Some(rx);
    if let Err(error) = std::thread::Builder::new()
        .name("road-import-check".into())
        .spawn(move || {
            let mut messages = Vec::new();
            let mut changed = false;
            for (point, radius, label) in requests {
                match osm_ingest::queue_focus_roads_import(root.as_deref(), point, radius) {
                    Ok(true) => {
                        changed = true;
                        messages.push(format!("Queued focused road import for the {label}."));
                    }
                    Ok(false) => {}
                    Err(error) => messages.push(format!("Focused road import failed: {error}")),
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
        model.push_log(format!("Cannot start road import check: {error}"));
    }
}
