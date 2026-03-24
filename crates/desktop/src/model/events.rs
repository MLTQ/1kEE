use super::geo::GeoPoint;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventSeverity {
    Critical,
    Elevated,
    Advisory,
}

impl EventSeverity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Critical => "Critical",
            Self::Elevated => "Elevated",
            Self::Advisory => "Advisory",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Critical => egui::Color32::from_rgb(242, 90, 74),
            Self::Elevated => egui::Color32::from_rgb(255, 186, 73),
            Self::Advisory => egui::Color32::from_rgb(126, 208, 229),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FactalBrief {
    pub factal_id: String,
    pub severity_value: Option<i64>,
    pub occurred_at_raw: Option<String>,
    pub point_wkt: Option<String>,
    pub vertical: Option<String>,
    pub subvertical: Option<String>,
    pub topics: Vec<String>,
    pub content: Option<String>,
    pub raw_json_pretty: String,
}

#[derive(Clone, Debug)]
pub struct EventRecord {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub severity: EventSeverity,
    pub location_name: String,
    pub location: GeoPoint,
    pub source: String,
    pub occurred_at: String,
    pub factal_brief: Option<FactalBrief>,
}
