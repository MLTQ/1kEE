mod catalog;
mod detail;
mod literals;
pub mod markers;
mod refresh;
#[cfg(test)]
mod tests;
pub mod view;

use refresh::{Snapshot, Update};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

pub const PROJECT_URL: &str = "https://www.zackslab.com/buildout";
const RETRY: Duration = Duration::from_secs(3600);

enum Message {
    Cached(Arc<Snapshot>),
    Done(Result<(Arc<Snapshot>, Option<String>), String>),
}

/// Each enabled source owns at most one cache/network worker.
pub struct Source {
    pub enabled: bool,
    pub status: String,
    pub browser: view::Browser,
    snapshot: Option<Arc<Snapshot>>,
    path: PathBuf,
    worker: Option<(PathBuf, mpsc::Receiver<Message>)>,
    due: Instant,
}

impl Source {
    pub fn new(enabled: bool, path: PathBuf) -> Self {
        Self {
            enabled,
            status: "Waiting for first check".into(),
            browser: Default::default(),
            snapshot: None,
            path,
            worker: None,
            due: Instant::now(),
        }
    }

    pub fn set_path(&mut self, path: PathBuf) {
        if self.path != path {
            self.path = path;
            self.snapshot = None;
            self.due = Instant::now();
            self.status = "Waiting for first check".into();
            self.browser = Default::default();
        }
    }

    pub fn tick(&mut self) {
        while let Some((worker_path, rx)) = &self.worker {
            let result = rx.try_recv();
            if worker_path != &self.path {
                match result {
                    Ok(Message::Cached(_)) => continue,
                    Err(mpsc::TryRecvError::Empty) => break,
                    _ => {
                        self.worker = None;
                        continue;
                    }
                }
            }
            match result {
                Ok(Message::Cached(snapshot)) => {
                    self.snapshot = Some(snapshot);
                    self.status = "Checking for updates…".into();
                }
                Ok(Message::Done(Ok((snapshot, warning)))) => {
                    self.due = Instant::now() + snapshot.remaining();
                    self.snapshot = Some(snapshot);
                    self.status = warning.unwrap_or_else(|| "Up to date".into());
                    self.worker = None;
                }
                Ok(Message::Done(Err(error))) => self.failed(error),
                Err(mpsc::TryRecvError::Disconnected) => self.failed("worker stopped".into()),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if !self.enabled || self.worker.is_some() || Instant::now() < self.due {
            return;
        }
        self.status = "Checking for updates…".into();
        let path = self.path.clone();
        let previous = self.snapshot.clone();
        let (tx, rx) = mpsc::channel();
        self.worker = Some((path.clone(), rx));
        std::thread::spawn(move || {
            let result = load(&path, previous, &tx);
            let _ = tx.send(Message::Done(result));
            crate::app::request_repaint();
        });
    }

    fn failed(&mut self, reason: String) {
        self.worker = None;
        self.due = Instant::now() + RETRY;
        let state = if self.snapshot.is_some() {
            "Showing cached data"
        } else {
            "Data unavailable"
        };
        self.status = format!("{state} · {reason} · retry in 1h");
    }
}

pub fn cache_path(data_root: Option<&str>, asset_root: Option<&Path>) -> PathBuf {
    let root = data_root
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| asset_root.unwrap_or_else(|| Path::new(".")).join("Data"));
    root.join("ZacksLab/buildout.json")
}

fn load(
    path: &Path,
    previous: Option<Arc<Snapshot>>,
    tx: &mpsc::Sender<Message>,
) -> Result<(Arc<Snapshot>, Option<String>), String> {
    let previous = previous.or_else(|| refresh::read_cache(path).map(Arc::new));
    if let Some(snapshot) = &previous {
        if !snapshot.remaining().is_zero() {
            return Ok((Arc::clone(snapshot), None));
        }
        tx.send(Message::Cached(Arc::clone(snapshot)))
            .map_err(|_| "source closed")?;
        crate::app::request_repaint();
    }
    let snapshot = match refresh::fetch(previous.as_deref().map(refresh::validator))? {
        Update::Changed(snapshot) => Arc::new(snapshot),
        Update::Unchanged(tag) => {
            let mut snapshot = previous.ok_or("cannot revalidate a missing snapshot")?;
            refresh::revalidated(Arc::make_mut(&mut snapshot), tag);
            snapshot
        }
    };
    let warning = refresh::write_cache(path, &snapshot)
        .err()
        .map(|e| format!("Cache write failed · {e}"));
    Ok((snapshot, warning))
}
