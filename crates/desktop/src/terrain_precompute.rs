use crate::city_catalog::{self, CityEntry};
use crate::panels::world_map::srtm_focus_cache;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const PRECOMPUTE_RADIUS_MILES: f32 = 25.0;
const PRECOMPUTE_ZOOMS: &[f32] = &[3.0, 4.5, 6.5, 9.5, 12.0];

#[derive(Clone)]
pub struct PrecomputeJobSnapshot {
    pub city_label: String,
    pub ready_assets: usize,
    pub pending_assets: usize,
    pub total_assets: usize,
    pub state: PrecomputeJobState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PrecomputeJobState {
    Queued,
    Running,
    Completed,
}

struct PrecomputeJob {
    city_id: String,
    root: Option<PathBuf>,
    state: PrecomputeJobState,
}

struct PrecomputeManager {
    jobs: Vec<PrecomputeJob>,
}

pub fn queue_city(root: Option<&Path>, city: &CityEntry) {
    let mut guard = manager().lock().expect("precompute manager lock");
    let root = root.map(Path::to_path_buf);
    if guard
        .jobs
        .iter()
        .any(|job| job.city_id == city.id && job.root == root)
    {
        return;
    }

    guard.jobs.push(PrecomputeJob {
        city_id: city.id.to_owned(),
        root,
        state: PrecomputeJobState::Queued,
    });
}

pub fn tick(root: Option<&Path>) {
    let mut guard = manager().lock().expect("precompute manager lock");
    for job in &mut guard.jobs {
        if job.state == PrecomputeJobState::Completed {
            continue;
        }

        let Some(city) = city_catalog::by_id(&job.city_id) else {
            continue;
        };
        let effective_root = job.root.as_deref().or(root);
        let status = aggregate_status(effective_root, &city);

        if status.total_assets > 0 && status.ready_assets >= status.total_assets {
            job.state = PrecomputeJobState::Completed;
            continue;
        }

        job.state = if status.ready_assets == 0 && status.pending_assets == 0 {
            PrecomputeJobState::Queued
        } else {
            PrecomputeJobState::Running
        };

        for &zoom in PRECOMPUTE_ZOOMS {
            let radius = srtm_focus_cache::bucket_radius_for_target_radius_miles(
                zoom,
                PRECOMPUTE_RADIUS_MILES,
            );
            srtm_focus_cache::ensure_focus_contour_region(
                effective_root,
                city.location,
                zoom,
                radius,
            );
        }
    }
}

pub fn snapshots(root: Option<&Path>) -> Vec<PrecomputeJobSnapshot> {
    let guard = manager().lock().expect("precompute manager lock");
    let mut snapshots: Vec<_> = guard
        .jobs
        .iter()
        .filter_map(|job| {
            let city = city_catalog::by_id(&job.city_id)?;
            let effective_root = job.root.as_deref().or(root);
            let status = aggregate_status(effective_root, &city);
            Some(PrecomputeJobSnapshot {
                city_label: format!("{}, {}", city.name, city.country),
                ready_assets: status.ready_assets,
                pending_assets: status.pending_assets,
                total_assets: status.total_assets,
                state: if status.total_assets > 0 && status.ready_assets >= status.total_assets {
                    PrecomputeJobState::Completed
                } else {
                    job.state
                },
            })
        })
        .collect();

    snapshots.sort_by(|left, right| left.city_label.cmp(&right.city_label));
    snapshots
}

pub fn has_active_jobs(root: Option<&Path>) -> bool {
    snapshots(root)
        .iter()
        .any(|job| job.state != PrecomputeJobState::Completed)
}

fn aggregate_status(
    root: Option<&Path>,
    city: &CityEntry,
) -> srtm_focus_cache::FocusContourRegionStatus {
    let mut ready_assets = 0usize;
    let mut pending_assets = 0usize;
    let mut total_assets = 0usize;

    for &zoom in PRECOMPUTE_ZOOMS {
        let radius =
            srtm_focus_cache::bucket_radius_for_target_radius_miles(zoom, PRECOMPUTE_RADIUS_MILES);
        if let Some(status) =
            srtm_focus_cache::focus_contour_region_status(root, city.location, zoom, radius)
        {
            ready_assets += status.ready_assets;
            pending_assets += status.pending_assets;
            total_assets += status.total_assets;
        }
    }

    srtm_focus_cache::FocusContourRegionStatus {
        ready_assets,
        pending_assets,
        total_assets,
    }
}

fn manager() -> &'static Mutex<PrecomputeManager> {
    static MANAGER: OnceLock<Mutex<PrecomputeManager>> = OnceLock::new();
    MANAGER.get_or_init(|| Mutex::new(PrecomputeManager { jobs: Vec::new() }))
}
