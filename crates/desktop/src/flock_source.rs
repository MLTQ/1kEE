//! Local, user-downloaded Flock position inventory; independent of OSM ALPRs.
use crate::model::GeoPoint;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

#[path = "flock_tsv.rs"]
mod tsv;

pub const SOURCE_URL: &str = "https://flocksurveillance.org/data/cameras.tsv";

#[derive(Clone, Copy, Debug)]
pub struct Position {
    pub location: GeoPoint,
    pub in_service: bool,
}

#[derive(Default, Debug)]
pub struct Snapshot {
    pub positions: Vec<Position>,
    pub rows: usize,
    pub in_service: usize,
    pub skipped: usize,
}

/// UI-owned lifecycle; only a worker touches or parses the large TSV.
pub struct Source {
    pub show: bool,
    pub include_other_statuses: bool,
    pub status: String,
    pub snapshot: Arc<Snapshot>,
    pub revision: u64,
    path: Option<PathBuf>,
    worker: Option<mpsc::Receiver<Result<Snapshot, String>>>,
    reload_requested: bool,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            show: true,
            include_other_statuses: false,
            status: "local file pending".into(),
            snapshot: Arc::new(Snapshot::default()),
            revision: 0,
            path: None,
            worker: None,
            reload_requested: true,
        }
    }
}

impl Source {
    pub fn reload(&mut self) {
        self.reload_requested = true;
    }

    pub fn tick(&mut self, selected_root: Option<&Path>) {
        self.tick_path(source_path(selected_root));
    }

    fn tick_path(&mut self, path: PathBuf) {
        if self.path.as_ref() != Some(&path) {
            self.path = Some(path.clone());
            // A departed root's worker may finish, but cannot publish here.
            self.worker = None;
            self.snapshot = Arc::new(Snapshot::default());
            self.revision = self.revision.wrapping_add(1);
            self.reload_requested = true;
        }
        if let Some(worker) = &self.worker {
            let result = match worker.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("file reader stopped".into())),
            };
            if let Some(result) = result {
                self.worker = None;
                match result {
                    Ok(snapshot) => {
                        self.status = format!(
                            "{} positions · {} in service · {} skipped",
                            snapshot.positions.len(),
                            snapshot.in_service,
                            snapshot.skipped,
                        );
                        eprintln!("[1kEE] Flock: {} ({})", self.status, path.display());
                        self.snapshot = Arc::new(snapshot);
                        self.revision = self.revision.wrapping_add(1);
                    }
                    Err(error) => self.status = format!("Flock: {error}"),
                }
            }
        }
        if self.reload_requested && self.worker.is_none() {
            self.reload_requested = false;
            self.status = "loading local Flock positions…".into();
            let (sender, receiver) = mpsc::channel();
            self.worker = Some(receiver);
            std::thread::spawn(move || {
                let _ = sender.send(tsv::load(&path));
                crate::app::request_repaint();
            });
        }
    }

    pub fn visible_positions(&self) -> impl Iterator<Item = GeoPoint> + '_ {
        self.snapshot
            .positions
            .iter()
            .take(if self.show {
                self.snapshot.positions.len()
            } else {
                0
            })
            .filter(|position| position.in_service || self.include_other_statuses)
            .map(|position| position.location)
    }
}

fn source_path(selected_root: Option<&Path>) -> PathBuf {
    crate::settings_store::configured_data_root()
        .unwrap_or_else(|| {
            selected_root
                .map(Path::to_path_buf)
                .or_else(crate::settings_store::effective_asset_root)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Data")
        })
        .join("Flock/cameras.tsv")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_filter_and_visibility_keep_other_inventory_available() {
        let mut source = Source {
            snapshot: Arc::new(Snapshot {
                positions: vec![
                    Position {
                        location: GeoPoint {
                            lat: 40.0,
                            lon: -74.0,
                        },
                        in_service: true,
                    },
                    Position {
                        location: GeoPoint {
                            lat: 41.0,
                            lon: -74.0,
                        },
                        in_service: false,
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(source.visible_positions().count(), 1);
        source.include_other_statuses = true;
        assert_eq!(source.visible_positions().count(), 2);
        source.show = false;
        assert_eq!(source.visible_positions().count(), 0);
    }

    #[test]
    fn failed_reload_retains_good_data_and_a_new_root_clears_it() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("1kee-flock-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("cameras.tsv");
        std::fs::write(
            &path,
            "lat\tlon\tOBJECTID\tactive\tstatus\n40\t-74\t1\t1\tinService\n",
        )
        .unwrap();
        let mut source = Source::default();
        let drain = |source: &mut Source, path: &Path| {
            source.tick_path(path.to_owned());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while source.worker.is_some() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "reader did not complete"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
                source.tick_path(path.to_owned());
            }
        };
        drain(&mut source, &path);
        assert_eq!(source.visible_positions().count(), 1);
        let revision = source.revision;
        std::fs::write(&path, "broken download").unwrap();
        source.reload();
        drain(&mut source, &path);
        assert_eq!(source.visible_positions().count(), 1);
        assert_eq!(source.revision, revision);
        assert!(source.status.contains("missing lat"));
        drain(&mut source, &root.join("other.tsv"));
        assert_eq!(source.visible_positions().count(), 0);
        assert!(source.revision > revision);
        std::fs::remove_dir_all(root).unwrap();
    }
}
