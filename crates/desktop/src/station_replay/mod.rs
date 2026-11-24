mod locate;
mod source;

use crate::model::GeoPoint;
use crossbeam_channel::{Receiver, TryRecvError, bounded};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct StationId {
    pub network: String,
    pub code: String,
}

impl std::fmt::Display for StationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.network, self.code)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Station {
    pub point: GeoPoint,
    pub elevation_km: f64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Phase {
    P,
    S,
    Other,
}

#[derive(Clone)]
pub(crate) struct Pick {
    pub phase: String,
    pub time_ms: i64,
    pub used: bool,
}

impl Pick {
    pub fn family(&self) -> Phase {
        match self.phase.as_bytes().first() {
            Some(b'P') => Phase::P,
            Some(b'S') => Phase::S,
            _ => Phase::Other,
        }
    }
}

pub(crate) struct Observation {
    pub station: StationId,
    pub location: Station,
    pub picks: Vec<Pick>,
}

pub(crate) struct Origin {
    pub point: GeoPoint,
    pub time_ms: i64,
    pub depth_km: f64,
}

pub(crate) struct Fit {
    pub point: GeoPoint,
    pub time_offset_s: f64,
    pub rms_s: f64,
    pub stations: usize,
    pub offset_km: f64,
}

pub(crate) struct Recording {
    pub title: String,
    pub origin: Origin,
    pub stations: Vec<Observation>,
    pub total_stations: usize,
    pub fit: Result<Fit, &'static str>,
}

impl Recording {
    pub fn offset_s(&self, time_ms: i64) -> f64 {
        (time_ms - self.origin.time_ms) as f64 / 1000.0
    }

    fn timeline(&self) -> (f64, f64) {
        self.stations
            .iter()
            .flat_map(|station| station.picks.iter())
            .map(|pick| self.offset_s(pick.time_ms))
            .fold((-1.0_f64, 2.0_f64), |(start, end), t| {
                (start.min(t - 1.0), end.max(t + 3.0))
            })
    }
}

pub(crate) struct Playback {
    pub recording: Arc<Recording>,
    pub start: f64,
    pub end: f64,
    pub position: f64,
    pub speed: f64,
    pub playing: bool,
    last_frame: f64,
}

impl Playback {
    fn new(recording: Arc<Recording>, now: f64) -> Self {
        let (start, end) = recording.timeline();
        let speed = [1., 2., 5., 10., 20., 60.]
            .into_iter()
            .find(|speed| (end - start) / speed <= 35.0)
            .unwrap_or(60.0);
        Self {
            recording,
            start,
            end,
            position: start,
            speed,
            playing: true,
            last_frame: now,
        }
    }

    fn tick(&mut self, now: f64) {
        let elapsed = (now - self.last_frame).max(0.0);
        self.last_frame = now;
        if self.playing {
            self.position = (self.position + elapsed * self.speed).min(self.end);
            self.playing = self.position < self.end;
        }
    }

    pub fn toggle(&mut self) {
        if self.position >= self.end {
            self.position = self.start;
        }
        self.playing = !self.playing;
    }

    pub fn current_pick<'a>(&self, station: &'a Observation) -> Option<(&'a Pick, f64)> {
        let index = station
            .picks
            .partition_point(|pick| self.recording.offset_s(pick.time_ms) <= self.position);
        let pick = station.picks.get(index.checked_sub(1)?)?;
        Some((pick, self.position - self.recording.offset_s(pick.time_ms)))
    }
}

type LoadResult = Result<Recording, String>;

struct Pending {
    id: String,
    receiver: Receiver<LoadResult>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(crate) struct StationReplay {
    pub id_entry: String,
    pub status: Option<String>,
    pub playback: Option<Playback>,
    pending: Option<Pending>,
    cached: Option<(String, Arc<Recording>)>,
}

impl StationReplay {
    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }

    pub fn open(
        &mut self,
        event_id: &str,
        ctx: &egui::Context,
        now: f64,
    ) -> Option<(GeoPoint, f32)> {
        let event_id = event_id.trim();
        let event_id = event_id.strip_prefix("usgs-").unwrap_or(event_id);
        if event_id.is_empty()
            || event_id.len() > 48
            || !event_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            self.status = Some("Enter a USGS event ID, for example ci38457511.".into());
            return None;
        }

        self.stop();
        if let Some((_, recording)) = self.cached.as_ref().filter(|(id, _)| id == event_id) {
            let recording = Arc::clone(recording);
            return Some(self.begin(recording, now));
        }

        let id = event_id.to_owned();
        let (sender, receiver) = bounded(1);
        let ctx = ctx.clone();
        let request = id.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        std::thread::spawn(move || {
            let result = source::load(&request, &worker_cancelled);
            let _ = sender.send(result);
            ctx.request_repaint();
        });
        self.pending = Some(Pending {
            id,
            receiver,
            cancelled,
        });
        None
    }

    pub fn tick(&mut self, now: f64) -> Option<(GeoPoint, f32)> {
        if let Some(playback) = &mut self.playback {
            playback.tick(now);
        }

        let pending = self.pending.as_ref()?;
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Disconnected) => Err("Station replay worker stopped.".into()),
            Err(TryRecvError::Empty) => return None,
        };
        let id = pending.id.clone();
        self.pending = None;
        match result {
            Ok(recording) => {
                let recording = Arc::new(recording);
                self.cached = Some((id, Arc::clone(&recording)));
                Some(self.begin(recording, now))
            }
            Err(error) => {
                self.status = Some(error);
                None
            }
        }
    }

    fn begin(&mut self, recording: Arc<Recording>, now: f64) -> (GeoPoint, f32) {
        let focus = recording.origin.point;
        let mut ranges: Vec<_> = recording
            .stations
            .iter()
            .map(|station| locate::distance_km(focus, station.location.point))
            .collect();
        ranges.sort_unstable_by(f64::total_cmp);
        let range = ranges[ranges.len() * 3 / 4];
        let zoom = if range <= 180.0 {
            35.0
        } else if range <= 800.0 {
            12.0
        } else if range <= 2_000.0 {
            4.0
        } else {
            1.0
        };
        self.playback = Some(Playback::new(recording, now));
        (focus, zoom)
    }

    pub fn stop(&mut self) {
        self.playback = None;
        self.pending = None;
        self.status = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_triggers_at_observed_times_including_p_then_s() {
        let id = StationId {
            network: "CI".into(),
            code: "ABC".into(),
        };
        let point = GeoPoint {
            lat: 12.0,
            lon: -74.0,
        };
        let station = Observation {
            station: id,
            location: Station {
                point,
                elevation_km: 0.0,
            },
            picks: vec![
                Pick {
                    phase: "Pg".into(),
                    time_ms: 104_250,
                    used: true,
                },
                Pick {
                    phase: "Sg".into(),
                    time_ms: 108_500,
                    used: true,
                },
            ],
        };
        let recording = Arc::new(Recording {
            title: "quake".into(),
            origin: Origin {
                point,
                time_ms: 100_000,
                depth_km: 8.0,
            },
            stations: vec![station],
            total_stations: 1,
            fit: Err("too few stations"),
        });
        let mut playback = Playback::new(Arc::clone(&recording), 0.0);
        let station = &recording.stations[0];
        playback.position = 4.249;
        assert!(playback.current_pick(station).is_none());
        playback.position = 4.25;
        assert_eq!(playback.current_pick(station).unwrap().0.family(), Phase::P);
        playback.position = 8.50;
        assert_eq!(playback.current_pick(station).unwrap().0.family(), Phase::S);
        playback.tick(100.0);
        assert_eq!(playback.position, playback.end);
        assert!(!playback.playing);
        playback.toggle();
        assert_eq!(playback.position, playback.start);
    }
}
