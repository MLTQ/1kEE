use super::geo::GeoPoint;

/// A live AIS vessel position record fetched from AISStream.
#[derive(Clone, Debug)]
pub struct MovingTrack {
    /// 9-digit Maritime Mobile Service Identity.
    pub mmsi: u64,
    /// Vessel name as broadcast by the ship.
    pub name: String,
    pub location: GeoPoint,
    /// True heading 0–359 degrees; `None` when unknown (AIS value 511).
    pub heading_deg: Option<f32>,
    /// Speed over ground in knots.
    pub speed_knots: Option<f32>,
    /// Radio callsign from static data.
    pub callsign: Option<String>,
    /// IMO vessel number.
    pub imo: Option<u64>,
    /// Next port of call.
    pub destination: Option<String>,
    /// ETA as a human-readable string built from the AIS ETA struct.
    pub eta_str: Option<String>,
    /// AIS ship type code (70–79 = cargo, 80–89 = tanker, etc.).
    pub ship_type_code: Option<u32>,
    /// Draught in metres.
    pub draught_m: Option<f32>,
}

impl MovingTrack {
    /// Short descriptive label for the ship type.
    pub fn ship_type_label(&self) -> &'static str {
        match self.ship_type_code {
            Some(30)      => "Fishing",
            Some(36)      => "Sailing",
            Some(37)      => "Pleasure craft",
            Some(50)      => "Pilot",
            Some(51)      => "SAR",
            Some(52)      => "Tug",
            Some(53)      => "Port tender",
            Some(35)      => "Military",
            Some(60..=69) => "Passenger",
            Some(70..=79) => "Cargo",
            Some(80..=89) => "Tanker",
            _             => "Vessel",
        }
    }
}
