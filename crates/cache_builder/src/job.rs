use crate::args::{BboxCommand, ContoursBboxCommand};
use crate::contours::{ContourBuildProgress, GeoBounds, build_contour_tiles};
use crate::roads::{RoadBuildProgress, build_bbox_cache_with_progress};
use std::sync::mpsc::{self, Receiver};
use std::thread;

pub enum BuildJob {
    Bbox(BboxCommand),
    ContoursBbox(ContoursBboxCommand),
}

pub struct JobHandle {
    pub receiver: Receiver<BuildEvent>,
}

#[derive(Debug)]
pub enum BuildEvent {
    Progress(RoadBuildProgress),
    Finished(Result<String, String>),
}

pub fn spawn_job(job: BuildJob) -> JobHandle {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || match job {
        BuildJob::Bbox(command) => {
            let mut reporter = |progress: RoadBuildProgress| {
                let _ = tx.send(BuildEvent::Progress(progress));
            };
            let result = build_bbox_cache_with_progress(command, &mut reporter);
            let _ = tx.send(BuildEvent::Finished(result));
        }
        BuildJob::ContoursBbox(command) => {
            let bounds = GeoBounds {
                min_lat: command.min_lat,
                max_lat: command.max_lat,
                min_lon: command.min_lon,
                max_lon: command.max_lon,
            };
            let tmp_dir = command.tmp_dir.unwrap_or_else(|| {
                command
                    .cache_db_path
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join("srtm_focus_tmp")
            });
            let mut reporter = |p: ContourBuildProgress| {
                let _ = tx.send(BuildEvent::Progress(RoadBuildProgress {
                    stage:   p.stage,
                    fraction: p.fraction,
                    message: p.message,
                }));
            };
            let result = build_contour_tiles(
                &command.srtm_root,
                &command.cache_db_path,
                &tmp_dir,
                bounds,
                &command.zoom_buckets,
                &command.gdal_bin_dir,
                &mut reporter,
            );
            let _ = tx.send(BuildEvent::Finished(result));
        }
    });
    JobHandle { receiver: rx }
}

