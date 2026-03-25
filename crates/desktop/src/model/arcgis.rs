use super::geo::GeoPoint;

/// A layer discovered from an ArcGIS FeatureServer.
#[derive(Clone, Debug)]
pub struct ArcGisLayerDef {
    pub id: u32,
    pub name: String,
    pub geometry_type: String,
    pub color: egui::Color32,
}

/// A single feature fetched from an ArcGIS FeatureServer layer.
#[derive(Clone, Debug)]
pub struct ArcGisFeature {
    pub object_id: i64,
    pub source_url: String,
    pub layer_id: u32,
    pub location: GeoPoint,
    /// All non-empty attributes as ordered key-value string pairs.
    pub attributes: Vec<(String, String)>,
    /// Unix-millisecond timestamp if a Date field is present.
    pub date_ms: Option<i64>,
}

impl ArcGisFeature {
    pub fn get_attr(&self, key: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
    pub fn get_attr_i32(&self, key: &str) -> Option<i32> {
        self.get_attr(key)?.parse().ok()
    }
    /// True if any standard casualty field is > 0.
    pub fn has_casualties(&self) -> bool {
        [
            "CivilianKilled",
            "FriendlyKilled",
            "EnemyKilled",
            "CivilianWounded",
            "FriendlyWounded",
            "EnemyWounded",
        ]
        .iter()
        .any(|k| self.get_attr_i32(k).unwrap_or(0) > 0)
    }
}

/// A reference to an ArcGIS source stored in the model (lightweight; actual
/// cached data lives in arcgis_source static storage).
#[derive(Clone, Debug)]
pub struct ArcGisSourceRef {
    /// Canonical FeatureServer base URL.
    pub url: String,
    /// Which layer IDs the user has enabled.
    pub enabled_layer_ids: std::collections::HashSet<u32>,
}
