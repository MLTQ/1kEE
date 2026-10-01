//! Keep one displayed terrain generation until its replacement can cover it.
use super::{
    super::{
        contour_asset::{
            ContourPath, LocalContourLoad, LocalTileGeometry,
            residency::{Bounds, Viewport},
        },
        local_contour_pass::{self, LocalBatchId, LocalTileBatch},
    },
    lod,
};
use crate::model::ActiveBody;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Clone, Default)]
pub(super) struct Display {
    pub contours: Option<Arc<Vec<ContourPath>>>,
    pub batches: Vec<LocalTileBatch>,
    tiles: Vec<LocalTileGeometry>,
}

#[derive(Default)]
struct Handoff {
    identity: Option<(Option<PathBuf>, ActiveBody)>,
    display: Display,
}

static STATE: OnceLock<Mutex<Handoff>> = OnceLock::new();

pub(crate) fn reset() {
    if let Some(state) = STATE.get()
        && let Ok(mut state) = state.lock()
    {
        *state = Handoff::default();
    }
}

pub(super) fn select(
    root: Option<&Path>,
    view: Viewport,
    camera_zoom: f32,
    candidate: &LocalContourLoad,
    batches: Vec<LocalTileBatch>,
    gpu: bool,
) -> Display {
    let Ok(mut state) = STATE.get_or_init(Default::default).lock() else {
        return Display::default();
    };
    let uploaded = !gpu
        || (batches.len() == candidate.tiles.len()
            && local_contour_pass::batches_uploaded(&batches));
    state.update(
        root,
        view,
        camera_zoom,
        candidate,
        if gpu { batches } else { Vec::new() },
        uploaded,
    );
    if !gpu {
        state.display.batches.clear();
    }
    let keep = if gpu {
        state
            .display
            .tiles
            .iter()
            .chain(&candidate.tiles)
            .map(|t| t.id)
            .collect()
    } else {
        HashSet::new()
    };
    local_contour_pass::retain_instances(&keep);
    state.display.clone()
}

impl Handoff {
    fn update(
        &mut self,
        root: Option<&Path>,
        view: Viewport,
        camera_zoom: f32,
        candidate: &LocalContourLoad,
        batches: Vec<LocalTileBatch>,
        uploaded: bool,
    ) {
        let identity = (root.map(Path::to_path_buf), view.body);
        if self.identity.as_ref() != Some(&identity) {
            self.identity = Some(identity);
            self.display = Display::default();
        }
        self.display.prune(view);
        let published = candidate.tiles.iter().all(|tile| {
            candidate
                .ready_buckets
                .contains(&(tile.id.lat_bucket, tile.id.lon_bucket))
        });
        let preserves = preserves_coverage(&self.display.tiles, candidate, view, camera_zoom);
        // A cold viewport can display CPU arrivals progressively. Once it has a
        // map, neither the first decoded tile nor the first GPU upload can erase it.
        if self.display.tiles.is_empty() || (published && uploaded && preserves) {
            self.display = Display {
                contours: candidate.contours.clone(),
                batches: if uploaded { batches } else { Vec::new() },
                tiles: candidate.tiles.clone(),
            };
        }
    }
}

impl Display {
    fn prune(&mut self, view: Viewport) {
        self.tiles
            .retain(|tile| view.intersects(tile_bounds(tile, view.body)));
        let ids: HashSet<_> = self.tiles.iter().map(|t| t.id).collect();
        self.batches
            .retain(|batch| matches!(batch.id, LocalBatchId::Contour(id) if ids.contains(&id)));
        if self.tiles.is_empty() {
            self.contours = None;
            self.batches.clear();
        }
    }
}

fn tile_bounds(tile: &LocalTileGeometry, body: ActiveBody) -> Bounds {
    tile.bounds.unwrap_or_else(|| {
        let step = lod::step(body, lod::ZOOMS[tile.id.zoom_bucket.clamp(0, 10) as usize]);
        Bounds {
            min: [
                (tile.id.lon_bucket as f32 - 0.5) * step,
                (tile.id.lat_bucket as f32 - 0.5) * step,
                0.0,
            ],
            max: [
                (tile.id.lon_bucket as f32 + 0.5) * step,
                (tile.id.lat_bucket as f32 + 0.5) * step,
                0.0,
            ],
        }
    })
}

/// Compare geographic coverage, not tile counts: grids are not nested. Empty
/// decoded tiles are valid; a failed or unavailable source is not replacement
/// coverage. Limit checks to the visible ground window, so zooming into part of
/// a large old tile does not wait for children outside the camera.
fn preserves_coverage(
    old: &[LocalTileGeometry],
    next: &LocalContourLoad,
    view: Viewport,
    _camera_zoom: f32,
) -> bool {
    let step = lod::step(view.body, next.source_zoom);
    let decoded: HashSet<_> = next
        .tiles
        .iter()
        .filter(|t| {
            next.ready_buckets
                .contains(&(t.id.lat_bucket, t.id.lon_bucket))
        })
        .map(|t| (t.id.lat_bucket, t.id.lon_bucket))
        .collect();
    old.iter().all(|tile| {
        // An identical immutable tile needs no geographic replacement test.
        if next
            .tiles
            .iter()
            .any(|n| n.id == tile.id && Arc::ptr_eq(&n.contours, &tile.contours))
        {
            return true;
        }
        let bounds = tile_bounds(tile, view.body);
        let Some(visible) = visible_ground_bounds(view, bounds.min[2], bounds.max[2]) else {
            return false; // A degenerate camera cannot prove replacement coverage.
        };
        let min_lon = bounds.min[0].max(visible.min[0]);
        let max_lon = bounds.max[0].min(visible.max[0]);
        let min_lat = bounds.min[1].max(visible.min[1]);
        let max_lat = bounds.max[1].min(visible.max[1]);
        if min_lon > max_lon || min_lat > max_lat {
            return true;
        }
        let range = |lo: f32, hi: f32| (lo / step).round() as i32..=(hi / step).round() as i32;
        range(min_lat, max_lat).all(|lat| {
            range(min_lon, max_lon).all(|lon| {
                let overlap = Bounds {
                    min: [
                        min_lon.max((lon as f32 - 0.5) * step),
                        min_lat.max((lat as f32 - 0.5) * step),
                        bounds.min[2],
                    ],
                    max: [
                        max_lon.min((lon as f32 + 0.5) * step),
                        max_lat.min((lat as f32 + 0.5) * step),
                        bounds.max[2],
                    ],
                };
                !view.intersects(overlap)
                    || decoded.contains(&(lat, lon))
                    || next.culled_buckets.contains(&(lat, lon))
            })
        })
    })
}

/// Invert the exact affine projector at both height extremes. This clips checks
/// to a finite visible region without discarding elevated/distant lines at a
/// shallow pitch merely because they lie outside the nominal ground envelope.
fn visible_ground_bounds(view: Viewport, min_height: f32, max_height: f32) -> Option<Bounds> {
    let p = view.projection;
    let ground_y = -p.pitch_cos * p.ground_pitch_scale + p.pitch_sin * p.ground_depth_scale;
    if [ground_y, p.horizontal_scale, p.x_factor, p.y_factor]
        .iter()
        .any(|v| !v.is_finite() || v.abs() < 1e-8)
    {
        return None;
    }
    let mut bounds = Bounds {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    for corner in [
        view.rect.left_top(),
        view.rect.right_top(),
        view.rect.left_bottom(),
        view.rect.right_bottom(),
    ] {
        for height in [min_height, max_height] {
            let x = (corner.x - p.focus_center_x) / p.horizontal_scale;
            let elevation = height
                * p.z_factor
                * (p.pitch_sin * p.elevation_pitch_scale + p.pitch_cos * p.elevation_depth_scale);
            let y = (corner.y - p.focus_center_y + elevation) / ground_y;
            let lon = (x * p.yaw_cos + y * p.yaw_sin) / p.x_factor + p.focus_lon;
            let lat = (-x * p.yaw_sin + y * p.yaw_cos) / p.y_factor + p.focus_lat;
            for (i, v) in [lon, lat, height].into_iter().enumerate() {
                if !v.is_finite() {
                    return None;
                }
                bounds.min[i] = bounds.min[i].min(v);
                bounds.max[i] = bounds.max[i].max(v);
            }
        }
    }
    Some(bounds)
}

#[cfg(test)]
#[path = "handoff_tests.rs"]
mod tests;
