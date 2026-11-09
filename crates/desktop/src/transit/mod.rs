mod catalog;
mod feeds;
mod gtfs;
mod view;

use crate::model::GeoPoint;
pub(crate) use catalog::AGENCIES;
use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
pub(crate) use view::{Marker, controls, detail, draw, pick};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Location {
    Coordinate,
    Station,
    PredictedArrival,
}

struct Report {
    id: String,
    route: String,
    label: String,
    context: String,
    location: Location,
    point: GeoPoint,
    color: egui::Color32,
    time: u64,
    feed_time: bool,
    expires_at: Option<u64>,
    bus: bool,
}

impl Report {
    fn visible(&self, now: u64, buses: bool) -> bool {
        (buses || !self.bus)
            && self.time > 0
            && self.time <= now.saturating_add(30)
            && now.saturating_sub(self.time) <= 300
            && self.expires_at.is_none_or(|end| now <= end)
    }
}

fn now() -> u64 {
    crate::event_store::now_unix() as u64
}

fn point(lat: f64, lon: f64) -> Option<GeoPoint> {
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some(GeoPoint {
        lat: lat as f32,
        lon: lon as f32,
    })
}

fn color(hex: &str) -> egui::Color32 {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6
        && let Ok(rgb) = u32::from_str_radix(hex, 16)
    {
        return egui::Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
    }
    egui::Color32::from_rgb(247, 180, 89)
}

struct Station {
    name: String,
    point: GeoPoint,
}

#[derive(Default)]
struct Lookup {
    stops: HashMap<String, Station>,
    routes: HashMap<String, gtfs::Route>,
    trips: HashMap<String, gtfs::Trip>,
}

type CachedLookup = (Instant, Arc<Lookup>);
struct Update {
    lookup: Option<CachedLookup>,
    reports: Result<Vec<Report>>,
}

#[derive(Default)]
struct Source {
    enabled: bool,
    reports: Vec<Report>,
    error: Option<String>,
    lookup: Option<CachedLookup>,
    pending: Option<mpsc::Receiver<Update>>,
    next_poll: Option<Instant>,
}

impl Source {
    fn poll(&mut self, agency: &'static catalog::Agency, ctx: &egui::Context) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(update) => {
                    self.lookup = update.lookup;
                    match update.reports {
                        Ok(reports) => {
                            self.reports = reports;
                            self.error = None;
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("Refresh interrupted".into())
                }
            }
            self.pending = None;
            self.next_poll = Some(Instant::now() + Duration::from_secs(30));
        }
        if self.next_poll.is_some_and(|due| Instant::now() < due) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        let ctx = ctx.clone();
        let cached = self.lookup.clone();
        std::thread::spawn(move || {
            let _ = tx.send(feeds::refresh(agency, cached));
            ctx.request_repaint();
        });
    }
}

pub(crate) struct Transit {
    pub enabled: bool,
    buses: bool,
    sources: Vec<Source>,
    selected: Option<(usize, String)>,
}

impl Default for Transit {
    fn default() -> Self {
        let mut sources: Vec<_> = AGENCIES.iter().map(|_| Source::default()).collect();
        sources[0].enabled = true;
        Self {
            enabled: false,
            buses: false,
            sources,
            selected: None,
        }
    }
}

impl Transit {
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.enabled {
            for (source, agency) in self.sources.iter_mut().zip(AGENCIES) {
                if source.enabled {
                    source.poll(agency, ctx);
                }
            }
            if self.sources.iter().any(|s| s.enabled) {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }
    }
}

#[cfg(test)]
mod tests;
