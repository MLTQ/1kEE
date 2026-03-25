use super::geo::GeoPoint;

/// Classification of a flight derived from its ICAO callsign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightCategory {
    /// Scheduled passenger airline — 3-letter ICAO code + flight number (UAL123, BAW456).
    Airline,
    /// Known cargo operators (FedEx, UPS, DHL, etc.).
    Cargo,
    /// Military or government callsigns.
    Military,
    /// General aviation — registration-style callsign (N123AB, G-ABCD).
    GA,
    /// No callsign or unrecognized pattern.
    Unknown,
}

fn classify_callsign(cs: &str) -> FlightCategory {
    // Military prefixes first (takes priority over everything else).
    const MILITARY: &[&str] = &[
        "RCH", "SAM", "DUKE", "MAGMA", "REACH", "NAVY", "ARMY", "USAF", "NATO", "CLAM", "JAKE",
        "CASEY", "WOLF", "LOBO", "PETE", "DARKSTAR",
    ];
    if MILITARY.iter().any(|m| cs.starts_with(m)) {
        return FlightCategory::Military;
    }

    // Known cargo ICAO operator prefixes.
    const CARGO: &[&str] = &[
        "FDX", "UPS", "GTI", "ABX", "ATN", "DHL", "TNT", "CKS", "PAC", "NCR", "CLX", "KMF", "WOA",
        "AMC", "CGE", "AIJ",
    ];
    if CARGO.iter().any(|c| cs.starts_with(c)) {
        return FlightCategory::Cargo;
    }

    let bytes = cs.as_bytes();

    // ICAO airline pattern: exactly 3 uppercase letters then ≥1 digit.
    if bytes.len() >= 4
        && bytes[0].is_ascii_uppercase()
        && bytes[1].is_ascii_uppercase()
        && bytes[2].is_ascii_uppercase()
        && bytes[3].is_ascii_digit()
    {
        return FlightCategory::Airline;
    }

    // Registration-style: contains a hyphen (G-ABCD, VH-ABC, D-EABC).
    if cs.contains('-') {
        return FlightCategory::GA;
    }
    // US N-number: starts with 'N' followed by a digit.
    if bytes[0] == b'N' && bytes.len() >= 2 && bytes[1].is_ascii_digit() {
        return FlightCategory::GA;
    }

    FlightCategory::Unknown
}

/// A live ADS-B flight position record fetched from OpenSky Network.
#[derive(Clone, Debug)]
pub struct FlightTrack {
    /// ICAO 24-bit transponder address (hex string, e.g. "a1b2c3").
    pub icao24: String,
    /// Flight callsign / flight number, if broadcast (e.g. "UAL123").
    pub callsign: Option<String>,
    /// Country of registration.
    pub origin_country: Option<String>,
    pub location: GeoPoint,
    /// Barometric altitude in metres; `None` when unavailable.
    pub baro_altitude_m: Option<f32>,
    /// True when the aircraft is reporting itself as on the ground.
    pub on_ground: bool,
    /// Ground speed in knots; `None` when unavailable.
    pub speed_knots: Option<f32>,
    /// Track angle in degrees clockwise from north; `None` when unavailable.
    pub heading_deg: Option<f32>,
    /// Vertical rate in feet per minute (positive = climbing); `None` when unavailable.
    pub vertical_rate_fpm: Option<f32>,
}

impl FlightTrack {
    /// Short display label — callsign if known, otherwise ICAO24.
    pub fn label(&self) -> &str {
        self.callsign.as_deref().unwrap_or(&self.icao24)
    }

    /// Altitude formatted for display (feet, rounded to nearest 100).
    pub fn altitude_label(&self) -> String {
        match self.baro_altitude_m {
            Some(m) if m > 0.0 => {
                let ft = (m * 3.280_84 / 100.0).round() as i32 * 100;
                format!("{ft} ft")
            }
            _ => "—".into(),
        }
    }

    /// Vertical trend symbol.
    pub fn trend_symbol(&self) -> &'static str {
        match self.vertical_rate_fpm {
            Some(r) if r > 100.0 => "↑",
            Some(r) if r < -100.0 => "↓",
            _ => "→",
        }
    }

    /// Classify this flight based on its callsign.
    pub fn category(&self) -> FlightCategory {
        match self.callsign.as_deref().filter(|s| !s.is_empty()) {
            Some(cs) => classify_callsign(cs),
            None => FlightCategory::Unknown,
        }
    }
}
