//! Coalesced terrain composition and atomic CPU/GPU display publication.
use super::composition::{self, Frame, Piece, Source};
use crate::model::ActiveBody;
use crate::panels::world_map::{
    contour_asset::{ContourPath, LocalContourLoad, LocalTileGeometry, residency::Viewport},
    local_contour_pass::{self, LocalBatchId, LocalTileBatch, LocalTileId},
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Default)]
pub(super) struct Display {
    frame_id: u64,
    pub contours: Option<Arc<Vec<ContourPath>>>,
    pub batches: Vec<LocalTileBatch>,
    tiles: Vec<LocalTileGeometry>,
}

#[derive(Default)]
struct Handoff {
    identity: Option<(Option<PathBuf>, ActiveBody)>,
    epoch: u64,
    generation: u64,
    tier: Option<i32>,
    sources: HashMap<LocalTileId, Source>,
    pieces: HashMap<LocalTileId, Piece>,
    revision: u64,
    built_revision: u64,
    working: Option<u64>,
    prepared: Option<Frame>,
    display: Display,
}
static STATE: OnceLock<Mutex<Handoff>> = OnceLock::new();
static TICKET: AtomicU64 = AtomicU64::new(1);

pub(crate) fn reset() {
    if let Some(state) = STATE.get()
        && let Ok(mut state) = state.lock()
    {
        state.clear();
    }
}

/// Prepare a geographic composition, independently of the current camera scale.
/// A prepared frame remains stable until its GPU upload finishes, so subsequent
/// read arrivals cannot perpetually replace work that is still being uploaded.
pub(super) fn prepare(
    root: Option<&Path>,
    view: Viewport,
    load: &LocalContourLoad,
    ctx: &egui::Context,
) -> Frame {
    let mut state = STATE.get_or_init(Default::default).lock().unwrap();
    state.observe(root, view, load);
    if state.working.is_none() && state.prepared.is_none() && state.revision != state.built_revision
    {
        let ticket = TICKET.fetch_add(1, Ordering::Relaxed);
        let (epoch, generation, revision) = (state.epoch, state.generation, state.revision);
        let sources = state.sources.values().cloned().collect();
        let previous = state.pieces.clone();
        state.working = Some(ticket);
        let ctx = ctx.clone();
        let worker_ctx = ctx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("terrain-lod-composition".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    composition::compose(view.body, sources, previous)
                }));
                if let Ok(mut state) = STATE.get().unwrap().lock() {
                    state.finish(ticket, epoch, generation, revision, result.ok());
                }
                worker_ctx.request_repaint();
            })
        {
            state.working = None;
            eprintln!("[1kEE] failed to spawn terrain LOD composition: {error}");
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
    state.candidate()
}

pub(super) fn select(
    view: Viewport,
    candidate: Frame,
    batches: Vec<LocalTileBatch>,
    gpu: bool,
    ctx: &egui::Context,
) -> Display {
    let Ok(mut state) = STATE.get_or_init(Default::default).lock() else {
        return Display::default();
    };
    let uploaded = !gpu
        || (batches.len() == candidate.tiles.len()
            && local_contour_pass::batches_uploaded(&batches));
    state.publish(candidate, batches, gpu, uploaded);
    if state.revision != state.built_revision && state.prepared.is_none() {
        ctx.request_repaint();
    }
    state.display.prune(view);
    let keep = if gpu {
        state
            .display
            .tiles
            .iter()
            .chain(state.prepared.iter().flat_map(|f| &f.tiles))
            .map(|t| t.id)
            .collect()
    } else {
        HashSet::new()
    };
    local_contour_pass::retain_instances(&keep);
    // Cold starts can use the composed CPU picture while the first upload is
    // staged. Subsequent updates always retain the previous displayed frame.
    if state.display.tiles.is_empty()
        && let Some(frame) = &state.prepared
    {
        Display {
            frame_id: frame.id,
            contours: frame.contours.clone(),
            tiles: frame.tiles.clone(),
            batches: Vec::new(),
        }
    } else {
        state.display.clone()
    }
}

impl Handoff {
    fn clear(&mut self) {
        self.identity = None;
        self.epoch = self.epoch.wrapping_add(1);
        self.generation = self.generation.wrapping_add(1);
        self.tier = None;
        self.sources.clear();
        self.pieces.clear();
        self.revision = self.revision.wrapping_add(1);
        self.built_revision = self.revision;
        self.prepared = None;
        self.display = Display::default();
        // Keep the worker slot occupied until a stale worker returns. Rapid
        // resets/body switches cannot spawn an unbounded composition fan-out.
    }

    fn observe(&mut self, root: Option<&Path>, view: Viewport, load: &LocalContourLoad) {
        let identity = (root.map(Path::to_path_buf), view.body);
        if self.identity.as_ref() != Some(&identity) {
            self.clear();
            self.identity = Some(identity);
        }
        self.display.prune(view);
        if let Some(frame) = &mut self.prepared {
            frame
                .tiles
                .retain(|t| view.intersects(composition::visible_bounds(view.body, t)));
            if frame.tiles.is_empty() {
                frame.contours = None;
            }
        }
        let before = self.sources.len();
        self.sources
            .retain(|_, s| view.intersects(composition::visible_bounds(view.body, &s.tile)));
        if self.sources.len() != before {
            self.revision = self.revision.wrapping_add(1);
        }
        self.pieces.retain(|id, _| self.sources.contains_key(id));
        let tier = match view.body {
            ActiveBody::Earth => {
                super::super::srtm_focus_cache::zoom_bucket_for_zoom(load.source_zoom)
            }
            _ => {
                super::super::srtm_focus_cache::zoom::lunar_spec_for_zoom(load.source_zoom)
                    .zoom_bucket
            }
        };
        if self.tier != Some(tier) {
            self.tier = Some(tier);
            self.generation = self.generation.wrapping_add(1);
            self.prepared = None;
            // Reversing the zoom can reuse resident data of this source tier.
            for source in self
                .sources
                .values_mut()
                .filter(|s| s.tile.id.zoom_bucket == tier)
            {
                source.priority = self.generation;
            }
            self.revision = self.revision.wrapping_add(1);
        }
        for tile in &load.tiles {
            if !load
                .ready_buckets
                .contains(&(tile.id.lat_bucket, tile.id.lon_bucket))
                || !view.intersects(composition::visible_bounds(view.body, tile))
            {
                continue;
            }
            if self.sources.get(&tile.id).is_some_and(|s| {
                Arc::ptr_eq(&s.tile.contours, &tile.contours) && s.priority == self.generation
            }) {
                continue;
            }
            self.sources.insert(
                tile.id,
                Source {
                    tile: tile.clone(),
                    priority: self.generation,
                },
            );
            self.revision = self.revision.wrapping_add(1);
        }
    }

    fn finish(
        &mut self,
        ticket: u64,
        epoch: u64,
        generation: u64,
        revision: u64,
        result: Option<composition::Result>,
    ) {
        if self.working != Some(ticket) {
            return;
        }
        self.working = None;
        if self.epoch != epoch || self.generation != generation {
            return;
        }
        self.built_revision = revision;
        if let Some(mut result) = result {
            result.frame.id = ticket;
            if self.revision == revision {
                self.sources.retain(|id, _| result.pieces.contains_key(id));
            }
            self.pieces = result.pieces;
            self.prepared = Some(result.frame);
        }
    }

    fn candidate(&self) -> Frame {
        self.prepared.clone().unwrap_or_else(|| Frame {
            id: self.display.frame_id,
            tiles: self.display.tiles.clone(),
            contours: self.display.contours.clone(),
        })
    }

    fn publish(
        &mut self,
        candidate: Frame,
        batches: Vec<LocalTileBatch>,
        gpu: bool,
        uploaded: bool,
    ) {
        if uploaded {
            if self.prepared.as_ref().is_some_and(|f| f.id == candidate.id) {
                self.prepared = None;
            }
            self.display = Display {
                frame_id: candidate.id,
                contours: candidate.contours,
                tiles: candidate.tiles,
                batches: if gpu { batches } else { Vec::new() },
            };
        } else if !gpu {
            self.display.batches.clear();
        }
    }
}

impl Display {
    fn prune(&mut self, view: Viewport) {
        self.tiles
            .retain(|t| view.intersects(composition::visible_bounds(view.body, t)));
        let ids: HashSet<_> = self.tiles.iter().map(|t| t.id).collect();
        self.batches
            .retain(|b| matches!(b.id, LocalBatchId::Contour(id) if ids.contains(&id)));
        if self.tiles.is_empty() {
            self.contours = None;
            self.batches.clear();
        }
    }
}

#[cfg(test)]
#[path = "handoff_tests.rs"]
mod tests;
