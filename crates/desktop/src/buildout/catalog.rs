use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Campus {
    pub id: String,
    pub name: String,
    pub operator: String,
    #[serde(default)]
    pub tenants: Vec<String>,
    pub region: Option<String>,
    pub notes: Option<String>,
    pub first_power: Option<String>,
    pub full_build: Option<String>,
    pub city: String,
    pub country: String,
    pub lat: f32,
    pub lng: f32,
    pub precision: String,
    pub status: String,
    pub confidence: String,
    pub last_verified: String,
    pub mw: Capacity,
    pub sources: Vec<Citation>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Capacity {
    pub operational: Option<f64>,
    pub construction: Option<f64>,
    pub planned: Option<f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Citation {
    pub url: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub country: String,
    pub region: Option<String>,
    pub locality: Option<String>,
    pub level: String,
    pub kind: String,
    pub severity: u8,
    pub status: String,
    pub date: String,
    pub effective: Option<String>,
    pub expires: Option<String>,
    pub lat: Option<f32>,
    pub lng: Option<f32>,
    pub sources: Vec<Citation>,
    pub last_verified: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct News {
    pub id: String,
    pub title: String,
    pub date: String,
    pub summary: String,
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(default)]
    pub rules: Vec<String>,
    pub sources: Vec<Citation>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    pub campuses: Vec<Campus>,
    pub rules: Vec<Rule>,
    pub news: Vec<News>,
}

fn unique<'a>(mut ids: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = HashSet::new();
    ids.all(|id| !id.is_empty() && seen.insert(id))
}

fn position(lat: f32, lng: f32) -> bool {
    (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lng)
}

impl Catalog {
    pub(super) fn valid(&self) -> bool {
        !self.campuses.is_empty()
            && !self.rules.is_empty()
            && !self.news.is_empty()
            && unique(self.campuses.iter().map(|c| c.id.as_str()))
            && unique(self.rules.iter().map(|r| r.id.as_str()))
            && unique(self.news.iter().map(|n| n.id.as_str()))
            && self.campuses.iter().all(|c| {
                !c.name.trim().is_empty()
                    && position(c.lat, c.lng)
                    && [c.mw.operational, c.mw.construction, c.mw.planned]
                        .into_iter()
                        .flatten()
                        .all(|mw| mw.is_finite() && mw >= 0.0)
            })
            && self.rules.iter().all(|r| {
                !r.title.trim().is_empty()
                    && match (r.lat, r.lng) {
                        (Some(lat), Some(lng)) => position(lat, lng),
                        (None, None) => true,
                        _ => false,
                    }
            })
            && self.news.iter().all(|n| !n.title.trim().is_empty())
    }

    /// Classify by record shape, independent of module numbers and filenames.
    pub(super) fn ingest(&mut self, script: &str) -> bool {
        let mut contributed = false;
        for data in super::literals::arrays(script) {
            let Some(first) = data.as_array().and_then(|a| a.first()) else {
                continue;
            };
            if first.get("precision").is_some() && first.get("mw").is_some() {
                if let Ok(rows) = serde_json::from_value(data) {
                    self.campuses = rows;
                    contributed = true;
                }
            } else if first.get("severity").is_some() && first.get("level").is_some() {
                if let Ok(rows) = serde_json::from_value(data) {
                    self.rules = rows;
                    contributed = true;
                }
            } else if first.get("weight").is_some()
                && first.get("summary").is_some()
                && let Ok(rows) = serde_json::from_value(data)
            {
                self.news = rows;
                contributed = true;
            }
        }
        contributed
    }
}

// Accept only script assets in the publisher's static tree. Try page-specific
// chunks (typically near the end) first, without pinning a build hash.
pub(super) fn assets(html: &str) -> Vec<&str> {
    let mut assets = Vec::new();
    for tag in html
        .split("<script ")
        .skip(1)
        .filter_map(|rest| rest.split_once('>'))
    {
        let Some((_, rest)) = tag.0.split_once("src=\"") else {
            continue;
        };
        let Some((path, _)) = rest.split_once('"') else {
            continue;
        };
        if path.starts_with("/_next/static/")
            && path.ends_with(".js")
            && !path.contains("..")
            && path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
            && !assets.contains(&path)
        {
            assets.push(path);
        }
    }
    assets.reverse();
    assets
}
