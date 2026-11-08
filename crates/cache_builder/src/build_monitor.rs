//! Structured scan telemetry. Byte progress is source traversal, not map coverage.
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::Path,
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub phase: String,
    pub scanned_bytes: u64,
    pub total_bytes: u64,
    pub checkpoint_bytes: u64,
    pub nodes: u64,
    pub ways: u64,
    pub features: u64,
    /// Writes this run; repeated writes to a cell count again.
    pub cell_writes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub source: String,
    #[serde(default)]
    pub source_bytes: u64,
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub selected_layers: Vec<String>,
    pub output: String,
    pub state: String,
    pub stage: String,
    pub message: String,
    pub updated_unix: u64,
    pub elapsed_secs: f64,
    pub metrics: Option<Metrics>,
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn save(path: &Path, report: &Report) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}

#[derive(Default)]
pub struct Live {
    pub metrics: Option<Metrics>,
    pub tiles: u64,
    pub started: Option<Instant>,
    pub updated: Option<Instant>,
    pub finished: Option<Instant>,
    samples: VecDeque<(Instant, u64)>,
}
impl Live {
    pub fn start(&mut self) {
        *self = Self {
            started: Some(Instant::now()),
            ..Self::default()
        };
    }
    pub fn observe(&mut self, metrics: Option<&Metrics>) {
        let now = Instant::now();
        self.started.get_or_insert(now);
        self.updated = Some(now);
        let Some(metrics) = metrics else {
            self.metrics = None;
            self.samples.clear();
            return;
        };
        if self.metrics.as_ref().is_none_or(|old| {
            old.phase != metrics.phase || old.scanned_bytes > metrics.scanned_bytes
        }) {
            self.samples.clear();
        }
        self.samples.push_back((now, metrics.scanned_bytes));
        while self.samples.len() > 2
            && now.duration_since(self.samples[1].0) > Duration::from_secs(15)
        {
            self.samples.pop_front();
        }
        self.metrics = Some(metrics.clone());
    }
    pub fn elapsed(&self) -> Duration {
        self.started
            .map(|start| {
                self.finished
                    .unwrap_or_else(Instant::now)
                    .saturating_duration_since(start)
            })
            .unwrap_or_default()
    }
    pub fn rate(&self) -> Option<f64> {
        // An old rate must not imply a stalled or completed worker is still scanning.
        if self.finished.is_some() || self.updated?.elapsed() > Duration::from_secs(15) {
            return None;
        }
        let (a, x) = self.samples.front()?;
        let (b, y) = self.samples.back()?;
        rate(*x, *y, b.duration_since(*a).as_secs_f64())
    }
    pub fn eta(&self) -> Option<Duration> {
        let m = self.metrics.as_ref()?;
        Some(Duration::from_secs_f64(
            m.total_bytes.saturating_sub(m.scanned_bytes) as f64 / self.rate()?,
        ))
    }
}
fn rate(start: u64, end: u64, seconds: f64) -> Option<f64> {
    (end > start && seconds >= 1.0).then(|| (end - start) as f64 / seconds)
}
pub fn bytes(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.2} GB", n as f64 / 1e9)
    } else {
        format!("{:.1} MB", n as f64 / 1e6)
    }
}
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}h {:02}m {:02}s", s / 3600, (s / 60) % 60, s % 60)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resumed_rates_count_only_new_bytes_and_reset_on_phase_change() {
        assert_eq!(rate(900, 1000, 2.0), Some(50.0));
        assert_eq!(rate(1000, 900, 2.0), None);
        assert_eq!(rate(900, 900, 2.0), None);
        let mut live = Live::default();
        live.observe(Some(&Metrics {
            phase: "nodes".into(),
            scanned_bytes: 900,
            ..Default::default()
        }));
        live.observe(Some(&Metrics {
            phase: "ways".into(),
            scanned_bytes: 950,
            ..Default::default()
        }));
        assert_eq!(live.samples.len(), 1);
        live.observe(None);
        assert!(live.metrics.is_none());
        assert!(live.rate().is_none());
    }
}
