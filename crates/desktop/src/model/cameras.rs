use super::geo::GeoPoint;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraConnectionState {
    Idle,
    Attempted,
    Reachable,
    Unreachable,
}

impl CameraConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Attempted => "attempted",
            Self::Reachable => "reachable",
            Self::Unreachable => "unreachable",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Idle => egui::Color32::from_gray(150),
            Self::Attempted => egui::Color32::from_rgb(126, 208, 229),
            Self::Reachable => egui::Color32::from_rgb(117, 201, 104),
            Self::Unreachable => egui::Color32::from_rgb(242, 90, 74),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CameraFeed {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub kind: String,
    pub location: GeoPoint,
    pub stream_url: String,
    pub last_seen: String,
    pub status: CameraConnectionState,
}

#[derive(Clone, Debug)]
pub struct NearbyCamera {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub kind: String,
    pub stream_url: String,
    pub last_seen: String,
    pub status: CameraConnectionState,
    pub distance_km: f32,
    pub location: GeoPoint,
}

/// Runtime liveness & reliability metrics per camera ID, updated by `CameraFeedPlayer`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CameraHealthRecord {
    pub last_ok_unix: Option<i64>,
    pub consecutive_failures: u32,
    pub latency_ms: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct CameraHealthTracker {
    records: HashMap<String, CameraHealthRecord>,
    revision: u64,
}

impl CameraHealthTracker {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn record_ok(&mut self, camera_id: &str, now_unix: i64, latency_ms: Option<u32>) {
        let entry = self.records.entry(camera_id.to_owned()).or_default();
        let recovered = entry.consecutive_failures > 0 || entry.last_ok_unix.is_none();
        entry.last_ok_unix = Some(now_unix);
        entry.consecutive_failures = 0;
        if let Some(ms) = latency_ms {
            entry.latency_ms = Some(ms);
        }
        if recovered {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn record_failure(&mut self, camera_id: &str) {
        let entry = self.records.entry(camera_id.to_owned()).or_default();
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        if entry.consecutive_failures <= 4 {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn get(&self, camera_id: &str) -> Option<&CameraHealthRecord> {
        self.records.get(camera_id)
    }

    /// Distance-equivalent ranking penalty (km) for failing or flaky cameras.
    pub fn penalty_km(&self, camera_id: &str) -> f32 {
        let Some(rec) = self.records.get(camera_id) else {
            return 0.0;
        };
        if rec.consecutive_failures >= 3 {
            // Heavy demotion below healthy nearby cameras without dropping permanently.
            5_000.0 + rec.consecutive_failures as f32 * 250.0
        } else if rec.consecutive_failures > 0 {
            rec.consecutive_failures as f32 * 85.0
        } else if rec.last_ok_unix.is_some() {
            // Verified working during this session gets a small ranking edge.
            -8.0
        } else {
            0.0
        }
    }
}

/// Reliability bonus (in km equivalent) for official public agency feeds vs ad-hoc directories.
pub fn provider_reliability_bonus_km(provider: &str) -> f32 {
    let p = provider.to_ascii_lowercase();
    if p.contains("caltrans")
        || p.contains("511")
        || p.contains("dot")
        || p.contains("digitraffic")
        || p.contains("tfl")
        || p.contains("usgs")
        || p.contains("geonet")
        || p.contains("noaa")
        || p.contains("faa")
        || p.contains("nps")
        || p.contains("alertcalifornia")
    {
        12.0
    } else if p.contains("windy") {
        4.0
    } else {
        0.0
    }
}
