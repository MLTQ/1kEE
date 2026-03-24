use crate::args::RoadsBboxCommand;
use crate::roads::{RoadBuildProgress, build_bbox_cache_with_progress};
use std::sync::mpsc::{self, Receiver};
use std::thread;

pub enum BuildJob {
    Roads(RoadsBboxCommand),
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
        BuildJob::Roads(command) => {
            let mut reporter = |progress: RoadBuildProgress| {
                let _ = tx.send(BuildEvent::Progress(progress));
            };
            let result =
                build_bbox_cache_with_progress(command, &mut reporter).map(|summary| summary);
            let _ = tx.send(BuildEvent::Finished(result));
        }
    });
    JobHandle { receiver: rx }
}
