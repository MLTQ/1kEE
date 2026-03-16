use crate::model::GeoPoint;

pub fn elevation(point: GeoPoint) -> f32 {
    let lat = point.lat.to_radians();
    let lon = point.lon.to_radians();

    let harmonic = 0.42
        + 0.18 * (lat * 2.1).sin() * (lon * 1.8).cos()
        + 0.11 * (lat * 5.4 + lon * 1.3).sin()
        + 0.08 * (lon * 3.7 - lat * 2.2).cos()
        + 0.05 * (lon * 9.1 + lat * 7.3).sin();

    let mountain_ranges = [
        (27.7, 86.9, 1.0, 10.5),
        (-32.7, -70.1, 0.9, 11.5),
        (46.5, 10.2, 0.55, 6.5),
        (35.4, 138.7, 0.48, 4.6),
        (61.0, -149.0, 0.52, 7.2),
        (-43.6, 170.2, 0.42, 5.6),
    ]
    .into_iter()
    .map(|(lat0, lon0, amplitude, sigma_deg)| {
        gaussian_peak(
            point,
            GeoPoint {
                lat: lat0,
                lon: lon0,
            },
            sigma_deg,
        ) * amplitude
    })
    .sum::<f32>();

    (harmonic + mountain_ranges).clamp(0.0, 1.6)
}

fn gaussian_peak(point: GeoPoint, center: GeoPoint, sigma_deg: f32) -> f32 {
    let lat_delta = point.lat - center.lat;
    let mut lon_delta = point.lon - center.lon;
    while lon_delta > 180.0 {
        lon_delta -= 360.0;
    }
    while lon_delta < -180.0 {
        lon_delta += 360.0;
    }

    let distance_sq = lat_delta * lat_delta + lon_delta * lon_delta;
    (-distance_sq / (2.0 * sigma_deg * sigma_deg)).exp()
}
