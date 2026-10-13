//! Public shaded-relief imagery. These images are never elevation samples.
use crate::model::GeoPoint;
use reqwest::Url;
use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

const WASHINGTON_SERVICE: &str =
    "https://gis.dnr.wa.gov/site3/rest/services/Geology/Lidar_Hillshade/MapServer/export";
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_IMAGE_PX: u32 = 2048;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    #[default]
    Usgs,
    WashingtonBareEarth,
    WashingtonSurface,
}

impl Source {
    pub const ALL: [Self; 3] = [
        Self::Usgs,
        Self::WashingtonBareEarth,
        Self::WashingtonSurface,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Usgs => "USGS 3DEP",
            Self::WashingtonBareEarth => "WA LiDAR: bare earth",
            Self::WashingtonSurface => "WA LiDAR: top surface",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Usgs => "Multidirectional relief shading from USGS elevation data.",
            Self::WashingtonBareEarth => {
                "Washington ground relief with buildings and vegetation removed. Coverage and survey dates vary."
            }
            Self::WashingtonSurface => {
                "Washington surface relief including trees and buildings. Coverage and survey dates vary."
            }
        }
    }

    /// A cheap extent screen; the returned image determines actual coverage.
    pub fn covers(self, bounds: Bounds) -> bool {
        let extent = match self {
            Self::Usgs => (-180.0, 15.0, -64.0, 72.0),
            // Slightly padded public service extent in geographic coordinates.
            _ => (-124.9, 45.3, -116.4, 49.1),
        };
        bounds.valid()
            && bounds.max_lon >= extent.0
            && bounds.min_lon <= extent.2
            && bounds.max_lat >= extent.1
            && bounds.min_lat <= extent.3
    }

    /// Called on the hillshade worker only. No credentials or disk cache.
    pub fn fetch(self, bounds: Bounds, width: u32, height: u32) -> Option<Vec<u8>> {
        if !crate::threedep::is_enabled() || !self.covers(bounds) {
            return None;
        }
        if self == Self::Usgs {
            return crate::threedep::fetch_hillshade_png(
                bounds.min_lat,
                bounds.min_lon,
                bounds.max_lat,
                bounds.max_lon,
                width,
                height,
            );
        }
        let url = washington_url(self, bounds, width, height)?;
        static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
        let client = CLIENT.get_or_init(|| {
            reqwest::blocking::Client::builder()
                .user_agent("1kEE terrain viewer")
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("hillshade HTTP client")
        });
        let response = client.get(url).send().ok()?.error_for_status().ok()?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_IMAGE_BYTES)
        {
            return None;
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        // ArcGIS can answer a service error as JSON/HTML with HTTP 200.
        (bytes.len() as u64 <= MAX_IMAGE_BYTES && bytes.starts_with(b"\x89PNG\r\n\x1a\n"))
            .then_some(bytes)
    }
}

fn washington_url(source: Source, bounds: Bounds, width: u32, height: u32) -> Option<Url> {
    if !source.covers(bounds)
        || !(16..=MAX_IMAGE_PX).contains(&width)
        || !(16..=MAX_IMAGE_PX).contains(&height)
    {
        return None;
    }
    let layer = match source {
        Source::WashingtonBareEarth => "show:1",
        Source::WashingtonSurface => "show:0",
        Source::Usgs => return None,
    };
    let mut url = Url::parse(WASHINGTON_SERVICE).ok()?;
    url.query_pairs_mut().extend_pairs([
        (
            "bbox",
            format!(
                "{},{},{},{}",
                bounds.min_lon, bounds.min_lat, bounds.max_lon, bounds.max_lat
            ),
        ),
        ("bboxSR", "4326".into()),
        ("imageSR", "4326".into()),
        ("size", format!("{width},{height}")),
        ("layers", layer.into()),
        ("format", "png32".into()),
        ("transparent", "true".into()),
        ("f", "image".into()),
    ]);
    Some(url)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min_lat: f32,
    pub min_lon: f32,
    pub max_lat: f32,
    pub max_lon: f32,
}

impl Bounds {
    pub fn around(center: GeoPoint, half_extent_deg: f32) -> Self {
        Self {
            min_lat: center.lat - half_extent_deg,
            min_lon: center.lon - half_extent_deg,
            max_lat: center.lat + half_extent_deg,
            max_lon: center.lon + half_extent_deg,
        }
    }

    fn valid(self) -> bool {
        [self.min_lat, self.min_lon, self.max_lat, self.max_lon]
            .iter()
            .all(|v| v.is_finite())
            && self.min_lat >= -90.0
            && self.max_lat <= 90.0
            && self.min_lon >= -180.0
            && self.max_lon <= 180.0
            && self.min_lat < self.max_lat
            && self.min_lon < self.max_lon
    }

    pub fn contains_with_margin(&self, other: &Self, fraction: f32) -> bool {
        let lat_margin = (self.max_lat - self.min_lat) * fraction;
        let lon_margin = (self.max_lon - self.min_lon) * fraction;
        other.min_lat >= self.min_lat - lat_margin
            && other.max_lat <= self.max_lat + lat_margin
            && other.min_lon >= self.min_lon - lon_margin
            && other.max_lon <= self.max_lon + lon_margin
    }
}

#[cfg(test)]
#[path = "hillshade_source_tests.rs"]
mod tests;
