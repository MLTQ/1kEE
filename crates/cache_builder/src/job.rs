use crate::args::{BboxCommand, ContoursBboxCommand, PlanetAllCommand};
use crate::contours::{ContourBuildProgress, GeoBounds, build_contour_tiles};
use crate::lunar::LunarBuildCommand;
use crate::mars::MarsBuildCommand;
use crate::planet_all::build_planet_cache_with_progress;
use crate::roads::{RoadBuildProgress, build_bbox_cache_with_progress};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub enum BuildJob {
    RecoverContours(crate::contour_recovery::Command),
    Atlas(crate::atlas::Command),
    PackArchive(crate::archive_pack::Command),
    Bbox(BboxCommand),
    ContoursBbox(ContoursBboxCommand),
    LunarContours(LunarBuildCommand),
    MarsContours(MarsBuildCommand),
    PlanetAll(PlanetAllCommand),
}

pub struct JobHandle {
    pub receiver: Receiver<BuildEvent>,
}

#[derive(Debug)]
pub enum BuildEvent {
    AtlasProgress(crate::atlas::Progress),
    Progress(RoadBuildProgress),
    Log(String),
    Finished(Result<String, String>),
    /// A contour tile finished building; carries (min_lat, max_lat, min_lon, max_lon).
    TileCompleted(f32, f32, f32, f32),
}

struct Notifier {
    tx: mpsc::Sender<BuildEvent>,
    wake: Option<egui::Context>,
    last_wake: Option<Instant>,
}

impl Notifier {
    fn new(tx: mpsc::Sender<BuildEvent>, wake: Option<egui::Context>) -> Self {
        Self {
            tx,
            wake,
            last_wake: None,
        }
    }

    fn send(&mut self, event: BuildEvent) {
        let urgent = matches!(event, BuildEvent::Finished(_));
        let _ = self.tx.send(event);
        if let Some(ctx) = &self.wake {
            let now = Instant::now();
            if urgent
                || self
                    .last_wake
                    .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(100))
            {
                self.last_wake = Some(now);
                ctx.request_repaint();
            }
        }
    }
}

pub fn spawn_job(job: BuildJob) -> JobHandle {
    spawn_job_with_wake(job, None)
}

pub fn spawn_job_with_wake(job: BuildJob, wake: Option<egui::Context>) -> JobHandle {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut notify = Notifier::new(tx, wake);
        match job {
            BuildJob::RecoverContours(command) => {
                let result = crate::contour_recovery::run(&command, &mut |message| {
                    notify.send(BuildEvent::Log(message));
                })
                .map_err(|e| format!("{e:#}"));
                notify.send(BuildEvent::Finished(result));
            }
            BuildJob::Atlas(command) => {
                let mut last_update: Option<Instant> = None;
                let result = crate::atlas::run(command, &mut |p| {
                    if last_update.is_some_and(|last| last.elapsed() < Duration::from_millis(100))
                        && p.state != "Completed"
                        && p.state != "Failed"
                    {
                        return;
                    }
                    last_update = Some(Instant::now());
                    notify.send(BuildEvent::Progress(crate::atlas::road_progress(&p)));
                    notify.send(BuildEvent::AtlasProgress(p));
                });
                notify.send(BuildEvent::Finished(result));
            }
            BuildJob::PackArchive(command) => {
                let mut last_log: Option<Instant> = None;
                let mut pending_log: Option<String> = None;
                let result = crate::archive_pack::run_with_progress(command, &mut |message| {
                    let now = Instant::now();
                    if last_log.is_none_or(|last| now.duration_since(last) >= Duration::from_millis(100)) {
                        last_log = Some(now);
                        pending_log = None;
                        notify.send(BuildEvent::Log(message));
                    } else {
                        pending_log = Some(message);
                    }
                });
                if let Some(last) = pending_log.take() {
                    notify.send(BuildEvent::Log(last));
                }
                notify.send(BuildEvent::Finished(result));
            }
            BuildJob::Bbox(command) => {
                let mut last_update: Option<Instant> = None;
                let mut last_stage = String::new();
                let mut reporter = |progress: RoadBuildProgress| {
                    let now = Instant::now();
                    if progress.stage != last_stage
                        || progress.fraction >= 1.0
                        || last_update.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(100))
                    {
                        last_update = Some(now);
                        last_stage.clone_from(&progress.stage);
                        notify.send(BuildEvent::Progress(progress));
                    }
                };
                let result = build_bbox_cache_with_progress(command, &mut reporter);
                notify.send(BuildEvent::Finished(result));
            }
            BuildJob::ContoursBbox(command) => {
                let bounds = GeoBounds {
                    min_lat: command.min_lat,
                    max_lat: command.max_lat,
                    min_lon: command.min_lon,
                    max_lon: command.max_lon,
                };
                let mut last_update: Option<Instant> = None;
                let mut last_stage = String::new();
                let mut reporter = |p: ContourBuildProgress| {
                    if p.is_error {
                        notify.send(BuildEvent::Log(p.message.clone()));
                    }
                    if let Some((min_lat, max_lat, min_lon, max_lon)) = p.tile_bounds {
                        notify.send(BuildEvent::TileCompleted(
                            min_lat, max_lat, min_lon, max_lon,
                        ));
                    }
                    let now = Instant::now();
                    if p.stage != last_stage
                        || p.fraction >= 1.0
                        || p.is_error
                        || last_update.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(100))
                    {
                        last_update = Some(now);
                        last_stage.clone_from(&p.stage);
                        notify.send(BuildEvent::Progress(RoadBuildProgress {
                            metrics: None,
                            stage: p.stage,
                            fraction: p.fraction,
                            message: p.message,
                        }));
                    }
                };
                let result = match command.engine {
                    crate::args::ContourEngine::Gpu => {
                        crate::gpu_terrain::build_bbox(&command, &mut reporter)
                    }
                    crate::args::ContourEngine::Native => {
                        crate::contours::build_contour_tiles_native(
                            &command.srtm_root,
                            &command.cache_db_path,
                            bounds,
                            &command.zoom_buckets,
                            &mut reporter,
                        )
                    }
                    crate::args::ContourEngine::Gdal => {
                        let tmp_dir = command.tmp_dir.unwrap_or_else(|| {
                            command
                                .cache_db_path
                                .parent()
                                .unwrap_or(std::path::Path::new("."))
                                .join("srtm_focus_tmp")
                        });
                        build_contour_tiles(
                            &command.srtm_root,
                            &command.cache_db_path,
                            &tmp_dir,
                            bounds,
                            &command.zoom_buckets,
                            &command.gdal_bin_dir,
                            &mut reporter,
                        )
                    }
                };
                notify.send(BuildEvent::Finished(result));
            }
            BuildJob::LunarContours(command) => {
                let mut last_update: Option<Instant> = None;
                let mut last_stage = String::new();
                let mut reporter = |p: ContourBuildProgress| {
                    if p.is_error {
                        notify.send(BuildEvent::Log(p.message.clone()));
                    }
                    if let Some((min_lat, max_lat, min_lon, max_lon)) = p.tile_bounds {
                        notify.send(BuildEvent::TileCompleted(
                            min_lat, max_lat, min_lon, max_lon,
                        ));
                    }
                    let now = Instant::now();
                    if p.stage != last_stage
                        || p.fraction >= 1.0
                        || p.is_error
                        || last_update.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(100))
                    {
                        last_update = Some(now);
                        last_stage.clone_from(&p.stage);
                        notify.send(BuildEvent::Progress(RoadBuildProgress {
                            metrics: None,
                            stage: p.stage,
                            fraction: p.fraction,
                            message: p.message,
                        }));
                    }
                };
                let result = crate::lunar::build_lunar_contour_tiles(command, &mut reporter);
                notify.send(BuildEvent::Finished(result));
            }
            BuildJob::MarsContours(command) => {
                let mut last_update: Option<Instant> = None;
                let mut last_stage = String::new();
                let mut reporter = |p: ContourBuildProgress| {
                    if p.is_error {
                        notify.send(BuildEvent::Log(p.message.clone()));
                    }
                    if let Some((min_lat, max_lat, min_lon, max_lon)) = p.tile_bounds {
                        notify.send(BuildEvent::TileCompleted(
                            min_lat, max_lat, min_lon, max_lon,
                        ));
                    }
                    let now = Instant::now();
                    if p.stage != last_stage
                        || p.fraction >= 1.0
                        || p.is_error
                        || last_update.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(100))
                    {
                        last_update = Some(now);
                        last_stage.clone_from(&p.stage);
                        notify.send(BuildEvent::Progress(RoadBuildProgress {
                            metrics: None,
                            stage: p.stage,
                            fraction: p.fraction,
                            message: p.message,
                        }));
                    }
                };
                let result = crate::mars::build_mars_contour_tiles(command, &mut reporter);
                notify.send(BuildEvent::Finished(result));
            }
            BuildJob::PlanetAll(command) => {
                let mut last_update: Option<Instant> = None;
                let mut last_stage = String::new();
                let mut reporter = |progress: RoadBuildProgress| {
                    let now = Instant::now();
                    if progress.stage != last_stage
                        || progress.fraction >= 1.0
                        || last_update.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(100))
                    {
                        last_update = Some(now);
                        last_stage.clone_from(&progress.stage);
                        notify.send(BuildEvent::Progress(progress));
                    }
                };
                let result = build_planet_cache_with_progress(command, &mut reporter);
                notify.send(BuildEvent::Finished(result));
            }
        }
    });
    JobHandle { receiver: rx }
}
