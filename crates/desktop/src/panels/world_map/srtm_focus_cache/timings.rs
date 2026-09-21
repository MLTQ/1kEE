//! Opt-in, per-stage measurements. No locks or clock reads per row when off.
use super::TileKey;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("ONEKEE_TERRAIN_TIMINGS").is_ok_and(|v| v == "1"))
}

pub fn record(tile: TileKey, stage: &str, elapsed: Duration, rows: u64, bytes: u64, outcome: &str) {
    eprintln!(
        "[terrain] tile={}/{}/{} stage={stage} ms={:.3} rows={rows} bytes={bytes} outcome={outcome}",
        tile.zoom_bucket,
        tile.lat_bucket,
        tile.lon_bucket,
        elapsed.as_secs_f64() * 1000.0,
    );
}

pub struct StageTimer {
    tile: TileKey,
    stage: &'static str,
    started: Option<Instant>,
}

impl StageTimer {
    pub fn new(tile: TileKey, stage: &'static str) -> Self {
        Self {
            tile,
            stage,
            started: enabled().then(Instant::now),
        }
    }

    pub fn clock(&self) -> Option<Instant> {
        self.started.map(|_| Instant::now())
    }

    pub fn elapsed(&self) -> Option<Duration> {
        self.started.map(|start| start.elapsed())
    }

    pub fn finish(mut self, rows: u64, bytes: u64) {
        if let Some(start) = self.started.take() {
            record(self.tile, self.stage, start.elapsed(), rows, bytes, "ok");
        }
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        if let Some(start) = self.started.take() {
            record(self.tile, self.stage, start.elapsed(), 0, 0, "aborted");
        }
    }
}
