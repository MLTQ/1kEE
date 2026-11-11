mod refresh;
#[cfg(test)]
mod tests;
mod view;

use crate::model::GeoJsonLayer;
use refresh::{Catalog, Product, WithWarning};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
pub(crate) use view::controls;

const PROJECT_URL: &str = "https://github.com/s2underground/GhostMaps";
const LAST_30: &str = "ArcGIS Data for ATAK (KMZs)/Common Intelligence Picture/30-Day Pulls/S2Underground_Common_Intelligence_Picture_Last_30_Days.kmz";
const RETRY: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    enabled: bool,
    selected: BTreeSet<String>,
    hidden_folders: BTreeMap<String, BTreeSet<String>>,
    labels: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            selected: BTreeSet::from([LAST_30.into()]),
            hidden_folders: BTreeMap::new(),
            labels: false,
        }
    }
}

pub(crate) fn cache_path(data_root: Option<&str>, asset_root: Option<&Path>) -> PathBuf {
    data_root
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| asset_root.unwrap_or_else(|| Path::new(".")).join("Data"))
        .join("GhostMaps/layers.sqlite")
}

fn export_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .trim_start_matches("S2Underground_")
        .replace('_', " ")
}

#[derive(Clone)]
enum Job {
    Catalog,
    Export { path: String, sha: String },
}

enum Data {
    Catalog(Catalog),
    Export(Product),
}

impl Job {
    fn run(&self, cache: &Path) -> Result<WithWarning<Data>, Box<dyn std::error::Error>> {
        let db = refresh::open(cache)?;
        let client = refresh::client()?;
        Ok(match self {
            Self::Catalog => {
                let (catalog, warning) = refresh::catalog(&db, &client, refresh::API)?;
                (Data::Catalog(catalog), warning)
            }
            Self::Export { path, sha } => {
                let (product, warning) = refresh::product(&db, &client, refresh::API, path, sha)?;
                (Data::Export(product), warning)
            }
        })
    }
}

type Reply = Result<WithWarning<Data>, String>;

pub(crate) struct Source {
    pub settings: Settings,
    cache: PathBuf,
    catalog: Option<Catalog>,
    catalog_due: Instant,
    catalog_warning: Option<String>,
    products: BTreeMap<String, Product>,
    errors: BTreeMap<String, (Instant, String)>,
    worker: Option<(Job, mpsc::Receiver<Reply>)>,
    search: String,
}

impl Source {
    pub fn new(settings: Settings, cache: PathBuf) -> Self {
        Self {
            settings,
            cache,
            catalog: None,
            catalog_due: Instant::now(),
            catalog_warning: None,
            products: BTreeMap::new(),
            errors: BTreeMap::new(),
            worker: None,
            search: String::new(),
        }
    }

    pub fn set_path(&mut self, cache: PathBuf) {
        if cache != self.cache {
            *self = Self::new(self.settings.clone(), cache);
        }
    }

    pub fn layers(&self) -> impl Iterator<Item = &[GeoJsonLayer]> {
        self.products
            .values()
            .filter(|_| self.settings.enabled)
            .map(|product| product.layers.as_slice())
    }

    fn apply_visibility(&mut self) {
        for (path, product) in &mut self.products {
            for layer in &mut product.layers {
                layer.visible = !self
                    .settings
                    .hidden_folders
                    .get(path)
                    .is_some_and(|set| set.contains(&layer.name));
                layer.show_labels = self.settings.labels;
            }
        }
    }

    fn next_job(&self, now: Instant) -> Option<Job> {
        if now >= self.catalog_due {
            return Some(Job::Catalog);
        }
        let files = &self.catalog.as_ref()?.files;
        for path in &self.settings.selected {
            let Some(sha) = files.get(path) else { continue };
            if self.products.get(path).is_some_and(|p| p.sha == *sha)
                || self.errors.get(path).is_some_and(|(due, _)| now < *due)
            {
                continue;
            }
            return Some(Job::Export {
                path: path.clone(),
                sha: sha.clone(),
            });
        }
        None
    }

    pub fn tick(&mut self) {
        if let Some((_, rx)) = &self.worker {
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => Err("GhostMaps worker stopped".into()),
            };
            let (job, _) = self.worker.take().unwrap();
            self.finish(job, result);
        }
        if !self.settings.enabled {
            return;
        }
        let Some(job) = self.next_job(Instant::now()) else {
            return;
        };
        let cache = self.cache.clone();
        let (tx, rx) = mpsc::channel();
        let worker_job = job.clone();
        std::thread::spawn(move || {
            let _ = tx.send(worker_job.run(&cache).map_err(|e| e.to_string()));
            crate::app::request_repaint();
        });
        self.worker = Some((job, rx));
    }

    fn finish(&mut self, job: Job, reply: Reply) {
        let now = Instant::now();
        let warning = match reply {
            Ok((Data::Catalog(catalog), warning)) => {
                self.catalog_due = now + catalog.remaining();
                self.products.retain(|p, _| catalog.files.contains_key(p));
                self.catalog = Some(catalog);
                warning
            }
            Ok((Data::Export(product), warning)) => {
                if let Job::Export { path, .. } = &job
                    && self.settings.selected.contains(path)
                {
                    self.products.insert(path.clone(), product);
                    self.apply_visibility();
                }
                warning
            }
            Err(e) => Some(e),
        };
        match job {
            Job::Catalog => {
                if warning.is_some() {
                    self.catalog_due = now + RETRY;
                }
                self.catalog_warning = warning;
            }
            Job::Export { path, .. } => {
                if let Some(error) = warning {
                    self.errors.insert(path, (now + RETRY, error));
                } else {
                    self.errors.remove(&path);
                }
            }
        }
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
        self.products
            .retain(|p, _| self.settings.selected.contains(p));
        self.apply_visibility();
    }
}
