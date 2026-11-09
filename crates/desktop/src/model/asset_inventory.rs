//! Asset scans must never delay creating the desktop window.
use super::AppModel;
use crate::{osm_ingest, settings_store, terrain_assets};
use std::{path::PathBuf, sync::mpsc, time::Instant};

struct Inventory {
    terrain: terrain_assets::TerrainInventory,
    osm: osm_ingest::OsmInventory,
    runtime: Result<PathBuf, String>,
    layout_error: Option<String>,
}
impl Inventory {
    fn load(root: Option<PathBuf>) -> Self {
        let started = Instant::now();
        eprintln!("[1kEE] Scanning terrain and OSM assets in the background");
        let layout_error = settings_store::ensure_default_asset_layout()
            .err()
            .map(|e| e.to_string());
        let terrain = terrain_assets::TerrainInventory::detect_from(root.as_deref());
        let runtime = osm_ingest::ensure_runtime_store(root.as_deref());
        let osm = osm_ingest::OsmInventory::detect_from(root.as_deref());
        eprintln!(
            "[1kEE] Asset inventory finished in {:.2}s",
            started.elapsed().as_secs_f64()
        );
        Self {
            terrain,
            osm,
            runtime,
            layout_error,
        }
    }
}

type Outcome = Result<Inventory, String>;
pub(super) struct Scan {
    root: Option<PathBuf>,
    generation: u64,
    refresh: bool,
    pending: Option<(u64, mpsc::Receiver<Outcome>)>,
}
impl Scan {
    pub(super) fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            generation: 0,
            refresh: true,
            pending: None,
        }
    }
    fn request(&mut self, root: Option<PathBuf>) {
        self.root = root;
        self.generation = self.generation.wrapping_add(1);
        self.refresh = true;
    }
    fn busy(&self) -> bool {
        self.refresh || self.pending.is_some()
    }
    fn poll(&mut self) -> Option<Outcome> {
        self.poll_with(Inventory::load)
    }
    fn poll_with(
        &mut self,
        load: impl FnOnce(Option<PathBuf>) -> Inventory + Send + 'static,
    ) -> Option<Outcome> {
        if let Some((generation, receiver)) = &self.pending {
            let result = match receiver.try_recv() {
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err("Asset inventory worker disconnected".into())
                }
                Ok(result) => result,
            };
            let current = *generation == self.generation;
            self.pending = None;
            if current {
                return Some(result);
            }
        }
        if self.refresh {
            self.refresh = false;
            let root = self.root.clone();
            let (tx, rx) = mpsc::channel();
            self.pending = Some((self.generation, rx));
            if let Err(error) = std::thread::Builder::new()
                .name("asset-inventory".into())
                .spawn(move || {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load(root)))
                            .map_err(|_| "Asset inventory worker panicked".to_string());
                    let _ = tx.send(result);
                    crate::app::request_repaint();
                })
            {
                self.pending = None;
                return Some(Err(error.to_string()));
            }
        }
        None
    }
}

impl AppModel {
    pub(crate) fn asset_inventory_pending(&self) -> bool {
        self.asset_scan.busy()
    }
    pub(crate) fn request_asset_inventory(&mut self) {
        self.asset_scan.request(self.selected_root.clone());
        self.terrain_inventory = terrain_assets::TerrainInventory {
            primary_runtime_source: "Scanning terrain assets…",
            ..Default::default()
        };
        self.osm_inventory = osm_ingest::OsmInventory {
            primary_runtime_source: "Scanning OSM assets…",
            ..Default::default()
        };
        self.push_log("Scanning terrain and OSM assets in the background…".into());
    }
    pub(crate) fn poll_asset_inventory(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.asset_scan.poll() {
            match result {
                Ok(result) => {
                    self.terrain_inventory = result.terrain;
                    self.osm_inventory = result.osm;
                    for line in self
                        .terrain_inventory
                        .status_lines()
                        .into_iter()
                        .chain(self.osm_inventory.status_lines())
                    {
                        self.push_log(line);
                    }
                    match result.runtime {
                        Ok(path) => {
                            self.push_log(format!("OSM runtime store ready: {}", path.display()))
                        }
                        Err(error) => {
                            self.push_log(format!("OSM runtime store unavailable: {error}"))
                        }
                    }
                    if let Some(error) = result.layout_error {
                        self.push_log(format!("Asset folder setup: {error}"));
                    }
                }
                Err(error) => {
                    self.terrain_inventory.primary_runtime_source =
                        "Terrain scan failed; see activity log";
                    self.osm_inventory.primary_runtime_source = "OSM scan failed; see activity log";
                    self.push_log(format!("Asset inventory failed: {error}"));
                }
            }
        }
        if self.asset_scan.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
#[path = "asset_inventory_tests.rs"]
mod tests;
