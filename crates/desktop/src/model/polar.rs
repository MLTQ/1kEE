//! Polar azimuthal equidistant chart. Plane distances are degrees of meridian arc.
use super::GeoPoint;

pub(crate) const POLAR_LATITUDE: f32 = 66.0;

pub(crate) fn pole_for(lat: f32) -> f32 {
    if lat.abs() >= POLAR_LATITUDE {
        lat.signum()
    } else {
        0.0
    }
}

/// East and north in the chart whose central meridian passes through `focus`.
pub(crate) fn offset(pole: f32, focus: GeoPoint, point: GeoPoint) -> egui::Vec2 {
    let angle = (point.lon - focus.lon).to_radians();
    let radius = 90.0 - pole * point.lat;
    // Writing north this way retains precision for nearby points at deep zoom.
    egui::vec2(
        radius * angle.sin(),
        point.lat - focus.lat + 2.0 * pole * radius * (0.5 * angle).sin().powi(2),
    )
}

/// Longitude is returned on the branch nearest the central meridian.
pub(crate) fn location(pole: f32, focus: GeoPoint, offset: egui::Vec2) -> GeoPoint {
    let radial = (90.0 - pole * focus.lat) - pole * offset.y;
    let radius = radial.hypot(offset.x);
    let angle = if radius == 0.0 {
        0.0
    } else {
        offset.x.atan2(radial).to_degrees()
    };
    GeoPoint {
        lat: pole * (90.0 - radius.min(180.0)),
        lon: focus.lon + angle,
    }
}
