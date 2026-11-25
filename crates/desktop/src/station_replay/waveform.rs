//! A selected station's vertical waveform, in uncalibrated instrument counts.
mod archive;

use super::{Pick, StationId, live::seedlink, source};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

const WINDOW: i64 = 120_000;
pub(crate) const BEFORE: i64 = 30_000;

#[derive(Clone)]
pub(crate) struct Request {
    pub station: StationId,
    pub arrival: Option<(Pick, String)>,
}

impl Request {
    pub fn range(&self, now: i64) -> (i64, i64) {
        let from = self
            .arrival
            .as_ref()
            .map_or(now - WINDOW, |(pick, _)| pick.time_ms - BEFORE);
        (from, from + WINDOW)
    }
}

pub(crate) fn station(text: &str) -> Option<StationId> {
    let upper = text.trim().to_ascii_uppercase();
    let (net, sta) = upper.split_once('.')?;
    source::station_id(Some(net), Some(sta))
}

pub(crate) type Sample = (i64, f64);

#[derive(Default)]
pub(crate) struct Trace {
    pub rate: f64,
    /// Preserve both extrema, in sample-time order, in each 50 ms bin.
    pub bins: BTreeMap<i64, (Sample, Sample)>,
}

impl Trace {
    fn add(&mut self, samples: impl IntoIterator<Item = Sample>, from: i64, to: i64) {
        for (ms, value) in samples {
            if !value.is_finite() || !(from..=to).contains(&ms) {
                continue;
            }
            let bin = self
                .bins
                .entry(ms.div_euclid(50))
                .or_insert(((ms, value), (ms, value)));
            if value < bin.0.1 {
                bin.0 = (ms, value);
            }
            if value > bin.1.1 {
                bin.1 = (ms, value);
            }
        }
    }
}

struct Block {
    channel: String,
    rate: f64,
    samples: Vec<Sample>,
}
enum Update {
    Samples(Block),
    Status(String),
}
struct Worker {
    stop: Arc<AtomicBool>,
    rx: Receiver<Update>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(crate) struct Seismogram {
    pub open: bool,
    pub entry: String,
    pub request: Option<Request>,
    pub traces: BTreeMap<String, Trace>,
    pub channel: String,
    pub channel_pinned: bool,
    pub status: String,
    worker: Option<Worker>,
}

impl Seismogram {
    pub(crate) fn select(&mut self, request: Request) {
        self.worker = None;
        self.entry = request.station.to_string();
        self.request = Some(request);
        self.channel.clear();
        self.channel_pinned = false;
        self.traces.clear();
        self.status.clear();
        self.open = true;
    }

    pub fn arrival(&mut self, station: StationId, pick: Pick, event: String) {
        self.select(Request {
            station,
            arrival: Some((pick, event)),
        });
    }

    pub fn live(&mut self, station: StationId) {
        self.select(Request {
            station,
            arrival: None,
        });
    }

    pub fn retry(&mut self) {
        if let Some(request) = self.request.clone() {
            self.select(request);
        }
    }

    pub fn tick(&mut self, ctx: &egui::Context, now: i64) {
        if !self.open {
            self.worker = None;
            return;
        }
        let Some(req) = &self.request else {
            return;
        };
        if self.worker.is_none() {
            let (tx, rx) = bounded(32);
            let stop = Arc::new(AtomicBool::new(false));
            self.worker = Some(Worker {
                stop: stop.clone(),
                rx,
            });
            let ctx = ctx.clone();
            let req = req.clone();
            std::thread::spawn(move || {
                loop {
                    load(&req, &stop, &tx, &ctx);
                    if req.arrival.is_some() || stop.load(Ordering::Relaxed) {
                        break;
                    }
                    super::live::wait(&stop, Duration::from_secs(15));
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                }
            });
        }
        let (from, to) = req.range(now);
        let rx = &self.worker.as_ref().unwrap().rx;
        while let Ok(update) = rx.try_recv() {
            match update {
                Update::Status(s) => self.status = s,
                Update::Samples(block) => {
                    let trace = self.traces.entry(block.channel.clone()).or_default();
                    trace.rate = block.rate;
                    trace.add(block.samples, from, to);
                    if self.channel.is_empty() && !trace.bins.is_empty() {
                        self.channel = block.channel;
                    }
                }
            }
        }
        if !self.channel_pinned {
            let reference = req
                .arrival
                .as_ref()
                .map(|(pick, _)| pick.time_ms.div_euclid(50));
            if let Some((name, _)) = self
                .traces
                .iter()
                .filter(|(_, t)| !t.bins.is_empty())
                .max_by_key(|(name, t)| {
                    let covers = reference.is_some_and(|bin| {
                        let margin = (20. / t.rate).ceil().max(1.) as i64;
                        t.bins.range(bin - margin..=bin + margin).next().is_some()
                    });
                    let fresh = t
                        .bins
                        .last_key_value()
                        .is_some_and(|(&last, _)| last * 50 >= now - 45_000);
                    let priority = match name.rsplit('.').next() {
                        Some("BHZ") => 3,
                        Some("HHZ") => 2,
                        Some("EHZ") => 1,
                        _ => 0,
                    };
                    (
                        covers,
                        req.arrival.is_none() && fresh,
                        priority,
                        t.bins.len(),
                    )
                })
            {
                self.channel.clone_from(name);
            }
        }
        if req.arrival.is_none() {
            for trace in self.traces.values_mut() {
                while trace
                    .bins
                    .first_key_value()
                    .is_some_and(|(&bin, _)| bin < from.div_euclid(50))
                {
                    trace.bins.pop_first();
                }
            }
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

fn load(req: &Request, stop: &AtomicBool, tx: &Sender<Update>, ctx: &egui::Context) {
    let send = |update| {
        if stop.load(Ordering::Relaxed) || tx.send(update).is_err() {
            return false;
        }
        ctx.request_repaint();
        true
    };
    let now = chrono::Utc::now().timestamp_millis();
    let (from, to) = req.range(now);
    let pick = req.arrival.as_ref().map(|(p, _)| p.time_ms);
    if pick.is_none() || from >= now - 15 * 60_000 {
        if !send(Update::Status("Connecting to station stream…".into())) {
            return;
        }
        let mut covers_pick = false;
        let mut first = true;
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())
            .and_then(|rt| {
                rt.block_on(seedlink::watch(
                    &req.station,
                    from,
                    pick.map(|_| to),
                    stop,
                    |record, start| {
                        use miniseed_rs::Samples;
                        if first {
                            first = false;
                            send(Update::Status(
                                if pick.is_some() {
                                    "Receiving recorded samples"
                                } else {
                                    "Live · EarthScope SeedLink"
                                }
                                .into(),
                            ));
                        }
                        let rate = record.sample_rate;
                        let values: Vec<f64> = match record.samples {
                            Samples::Int(v) => v.into_iter().map(f64::from).collect(),
                            Samples::Float(v) => v.into_iter().map(f64::from).collect(),
                            Samples::Double(v) => v,
                        };
                        let samples: Vec<Sample> = values
                            .into_iter()
                            .enumerate()
                            .map(|(i, x)| (start + (1000. * i as f64 / rate).round() as i64, x))
                            .collect();
                        covers_pick |= pick.is_some_and(|p| {
                            samples.last().is_some_and(|last| start <= p && p <= last.0)
                        });
                        send(Update::Samples(Block {
                            channel: format!("{}.{}", record.location, record.channel),
                            rate,
                            samples,
                        }))
                    },
                ))
            });
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if pick.is_none() {
            let status = result
                .err()
                .unwrap_or_else(|| "Station stream ended".into());
            send(Update::Status(format!("{status} · reconnecting")));
            return;
        }
        if covers_pick {
            send(Update::Status("Recorded station stream".into()));
            return;
        }
    }
    if !send(Update::Status("Loading archived samples…".into())) {
        return;
    }
    let result = archive::load(&req.station, from, to, |block| send(Update::Samples(block)));
    send(Update::Status(match result {
        Ok(true) => "EarthScope waveform archive".into(),
        Ok(false) => "No archived vertical samples for this station and time".into(),
        Err(e) => e,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graph_preserves_short_peaks_and_blank_intervals() {
        let mut trace = Trace::default();
        trace.add(
            [
                (1000, 0.),
                (1010, 90.),
                (1030, -60.),
                (1040, 0.),
                (4000, 1.),
                (8100, 7.),
            ],
            500,
            8000,
        );
        assert_eq!(trace.bins[&20], ((1030, -60.), (1010, 90.)));
        assert_eq!(trace.bins.len(), 2);
    }
    #[test]
    fn selected_arrival_renders_a_time_axis_and_arrival_marker() {
        let now = chrono::Utc::now().timestamp_millis();
        let mut chart = Seismogram::default();
        chart.arrival(
            station("IU.SSPA").unwrap(),
            Pick {
                time_ms: now,
                phase: "P".into(),
                used: true,
            },
            "us-test".into(),
        );
        let (tx, rx) = bounded(2);
        chart.worker = Some(Worker {
            stop: Arc::new(AtomicBool::new(false)),
            rx,
        });
        tx.send(Update::Samples(Block {
            channel: "00.BHZ".into(),
            rate: 20.,
            samples: vec![(now - 50, -25.), (now, 110.), (now + 50, -15.)],
        }))
        .unwrap();
        let ctx = egui::Context::default();
        let mut frame = || {
            ctx.run(egui::RawInput::default(), |ctx| {
                crate::panels::seismogram::render(ctx, &mut chart)
            })
        };
        frame();
        let shown = frame();
        let labels: Vec<_> = shown
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.job.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"Arrival") && labels.contains(&"Seismogram"));
        assert!(labels.iter().any(|s| s.contains("instrument counts")));
        assert!(
            labels
                .iter()
                .any(|s| s.contains("IU.SSPA") && s.contains("us-test"))
        );
    }

    #[test]
    #[ignore = "opens the public EarthScope SeedLink station stream"]
    fn selected_station_delivers_plottable_live_samples() {
        let mut chart = Seismogram::default();
        chart.live(station("IU.ANMO").unwrap());
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        while std::time::Instant::now() < deadline {
            chart.tick(&ctx, chrono::Utc::now().timestamp_millis());
            if chart.traces.values().any(|t| t.bins.len() > 5) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprintln!(
            "{} · {} · {} populated channels",
            chart.entry,
            chart.status,
            chart.traces.len()
        );
        assert!(chart.traces.values().any(|t| t.bins.len() > 5));
        chart.open = false;
        chart.tick(&ctx, chrono::Utc::now().timestamp_millis());
        assert!(chart.worker.is_none());
    }
}
