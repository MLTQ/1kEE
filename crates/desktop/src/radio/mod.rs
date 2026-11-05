//! Public radio metadata, geographic matching and stream validation.
pub(crate) mod catalog;
pub(crate) mod player;
pub(crate) mod saved;

use crate::model::{EventRecord, GeoPoint};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RadioArea {
    pub event_id: String,
    pub title: String,
    pub name: String,
    pub location: GeoPoint,
}

impl From<&EventRecord> for RadioArea {
    fn from(event: &EventRecord) -> Self {
        Self {
            event_id: event.id.clone(),
            title: event.title.clone(),
            name: event.location_name.clone(),
            location: event.location,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RadioKind {
    Police,
    Fire,
    Ems,
    PublicSafety,
    Aviation,
    Marine,
    Weather,
}

impl RadioKind {
    pub const ALL: [Self; 7] = [
        Self::Police,
        Self::Fire,
        Self::Ems,
        Self::PublicSafety,
        Self::Aviation,
        Self::Marine,
        Self::Weather,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Police => "Police",
            Self::Fire => "Fire",
            Self::Ems => "EMS",
            Self::PublicSafety => "Public safety",
            Self::Aviation => "Aviation",
            Self::Marine => "Marine",
            Self::Weather => "Weather",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RadioFeed {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub homepage: String,
    pub kinds: Vec<RadioKind>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub locality: String,
    /// A Radio Browser UUID only for entries obtained from its directory.
    pub directory_id: Option<String>,
}

impl RadioFeed {
    pub fn distance_km(&self, area: &RadioArea) -> Option<f64> {
        let (lat, lon) = (self.latitude?, self.longitude?);
        valid_coordinates(lat, lon)
            .then(|| distance_km(area.location.lat as f64, area.location.lon as f64, lat, lon))
    }
}

pub(crate) fn valid_coordinates(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
}

fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    6371.0 * 2.0 * a.clamp(0.0, 1.0).sqrt().asin()
}

/// Catalog URLs are untrusted. Never pass file/credential/local targets to a player.
pub(crate) fn stream_url(value: &str) -> Result<reqwest::Url, String> {
    if value.len() > 4096 {
        return Err("The radio stream URL is too long.".into());
    }
    let url = reqwest::Url::parse(value.trim())
        .map_err(|_| "Enter a complete HTTP or HTTPS stream URL.")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use a public HTTP(S) stream URL without embedded credentials.".into());
    }
    let host = url
        .host_str()
        .ok_or("The stream needs a public hostname.")?;
    let host = host
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host == "localhost"
        || !host.contains('.') && host.parse::<std::net::IpAddr>().is_err()
        || [
            ".localhost",
            ".local",
            ".internal",
            ".test",
            ".invalid",
            ".example",
        ]
        .iter()
        .any(|s| host.ends_with(s))
    {
        return Err("Use a public internet radio stream.".into());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        let public = match ip {
            std::net::IpAddr::V4(ip) => {
                !ip.is_private()
                    && !ip.is_loopback()
                    && !ip.is_link_local()
                    && !ip.is_unspecified()
                    && !ip.is_multicast()
                    && !ip.is_broadcast()
                    && !ip.is_documentation()
                    && ip.octets()[0] != 0
                    && ip.octets()[0] < 224
                    && !(ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
            }
            std::net::IpAddr::V6(ip) => {
                ip.to_ipv4_mapped().is_none()
                    && (ip.segments()[0] & 0xe000) == 0x2000
                    && !(ip.segments()[0] == 0x2001 && ip.segments()[1] == 0xdb8)
            }
        };
        if !public {
            return Err("Use a public internet radio stream.".into());
        }
    }
    if restricted_provider(&url) {
        return Err(
            "This provider needs an approved integration. Open its own website to listen.".into(),
        );
    }
    Ok(url)
}

pub(crate) fn restricted_provider(url: &reqwest::Url) -> bool {
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    [
        "broadcastify.com",
        "radioreference.com",
        "bcfy.io",
        "openmhz.com",
    ]
    .iter()
    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
