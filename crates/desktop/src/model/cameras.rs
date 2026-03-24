use super::geo::GeoPoint;

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
