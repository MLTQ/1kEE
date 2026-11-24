use super::{Fit, Observation, Origin};
use crate::model::GeoPoint;
use glam::{DVec2, DVec3};

const EARTH_KM: f64 = 6371.0;
const REGIONAL_KM: f64 = 150.0;
const MAX_SHIFT_KM: f64 = 60.0;
const INLIER_SECONDS: f64 = 3.0;
const MIN_STATIONS: usize = 4;

struct LocalFrame {
    up: DVec3,
    east: DVec3,
    north: DVec3,
}

impl LocalFrame {
    fn new(center: GeoPoint) -> Self {
        let (sin_lon, cos_lon) = f64::from(center.lon).to_radians().sin_cos();
        let up = unit(center);
        let east = DVec3::new(-sin_lon, cos_lon, 0.0);
        Self {
            up,
            east,
            north: up.cross(east),
        }
    }

    fn to_local(&self, point: GeoPoint) -> DVec2 {
        let unit = unit(point);
        let direction = DVec2::new(unit.dot(self.east), unit.dot(self.north));
        let angle = direction.length().atan2(unit.dot(self.up));
        direction.normalize_or_zero() * (EARTH_KM * angle)
    }

    fn to_geo(&self, xy: DVec2) -> GeoPoint {
        let direction = xy.normalize_or_zero();
        let (sin, cos) = (xy.length() / EARTH_KM).sin_cos();
        let unit = self.up * cos + (self.east * direction.x + self.north * direction.y) * sin;
        GeoPoint {
            lat: unit.z.clamp(-1.0, 1.0).asin().to_degrees() as f32,
            lon: unit.y.atan2(unit.x).to_degrees() as f32,
        }
    }
}

fn unit(point: GeoPoint) -> DVec3 {
    let (sin_lat, cos_lat) = f64::from(point.lat).to_radians().sin_cos();
    let (sin_lon, cos_lon) = f64::from(point.lon).to_radians().sin_cos();
    DVec3::new(cos_lat * cos_lon, cos_lat * sin_lon, sin_lat)
}

pub(super) fn distance_km(a: GeoPoint, b: GeoPoint) -> f64 {
    let a = unit(a);
    let b = unit(b);
    a.cross(b).length().atan2(a.dot(b)) * EARTH_KM
}

struct Datum {
    station: DVec2,
    depth_to_station: f64,
    velocity: f64,
    arrival: f64,
}

impl Datum {
    fn corrected_time(&self, source: DVec2) -> f64 {
        self.arrival - self.station.distance(source).hypot(self.depth_to_station) / self.velocity
    }
}

// A robust local point-source estimate. The depth is fixed to the catalog
// value because regional picks often have a strong depth/origin-time tradeoff.
pub(super) fn locate(origin: &Origin, observations: &[Observation]) -> Result<Fit, &'static str> {
    if !(0.0..=35.0).contains(&origin.depth_km) {
        return Err("Regional fit requires a catalog depth of 0–35 km.");
    }

    let frame = LocalFrame::new(origin.point);
    let mut data: Vec<_> = observations
        .iter()
        .filter_map(|station| {
            if distance_km(origin.point, station.location.point) > REGIONAL_KM {
                return None;
            }
            // One direct arrival per station avoids weighting a station twice.
            let (pick, velocity) = station
                .picks
                .iter()
                .filter(|pick| pick.used)
                .filter_map(|pick| match pick.phase.as_str() {
                    "P" | "Pg" => Some((pick, 6.0)),
                    "S" | "Sg" => Some((pick, 3.5)),
                    _ => None,
                })
                .min_by_key(|(pick, _)| pick.family())?;
            let arrival = (pick.time_ms - origin.time_ms) as f64 / 1000.0;
            (-1.0..=100.0).contains(&arrival).then(|| Datum {
                station: frame.to_local(station.location.point),
                depth_to_station: origin.depth_km + station.location.elevation_km,
                velocity,
                arrival,
            })
        })
        .collect();

    if data.len() < MIN_STATIONS {
        return Err("Need at least four nearby stations with direct P or S picks.");
    }

    let (source, time) = search(&data);
    data.retain(|datum| (datum.corrected_time(source) - time).abs() <= INLIER_SECONDS);
    if data.len() < MIN_STATIONS {
        return Err("Too few regional picks agree with the travel-time model.");
    }
    let (source, time) = search(&data);
    if source.length() >= MAX_SHIFT_KM - 0.5 {
        return Err("Best fit is outside the regional search area.");
    }
    if azimuth_gap(source, &data) > 220.0 || conditioning(source, &data) < 0.01 {
        return Err("Station geometry is too one-sided or collinear for a reliable fit.");
    }

    let rms_s = (data
        .iter()
        .map(|datum| (datum.corrected_time(source) - time).powi(2))
        .sum::<f64>()
        / data.len() as f64)
        .sqrt();
    if rms_s > 2.0 {
        return Err("Travel-time residuals are too large for the regional fit.");
    }
    Ok(Fit {
        point: frame.to_geo(source),
        time_offset_s: time,
        rms_s,
        stations: data.len(),
        offset_km: source.length(),
    })
}

fn search(data: &[Datum]) -> (DVec2, f64) {
    let mut best_point = DVec2::ZERO;
    let mut best_time = 0.0;
    let mut best_loss = f64::INFINITY;
    let mut corrected = Vec::with_capacity(data.len());
    for step in [15.0, 3.0, 0.6, 0.12] {
        let center = best_point;
        for y in -4..=4 {
            for x in -4..=4 {
                let source = center + DVec2::new(f64::from(x), f64::from(y)) * step;
                if source.length() > MAX_SHIFT_KM {
                    continue;
                }
                corrected.clear();
                corrected.extend(data.iter().map(|datum| datum.corrected_time(source)));
                let mid = corrected.len() / 2;
                let (_, middle, _) = corrected.select_nth_unstable_by(mid, f64::total_cmp);
                let time = *middle;
                let loss = corrected
                    .iter()
                    .map(|corrected| {
                        let residual = (corrected - time).abs();
                        // Huber loss limits the influence of mispicked arrivals.
                        if residual <= 1.0 {
                            0.5 * residual * residual
                        } else {
                            residual - 0.5
                        }
                    })
                    .sum::<f64>();
                if loss < best_loss {
                    (best_point, best_time, best_loss) = (source, time, loss);
                }
            }
        }
    }
    (best_point, best_time)
}

fn azimuth_gap(source: DVec2, data: &[Datum]) -> f64 {
    let mut azimuths: Vec<_> = data
        .iter()
        .filter_map(|datum| {
            let direction = datum.station - source;
            (direction.length() > 0.05).then(|| direction.y.atan2(direction.x).to_degrees())
        })
        .collect();
    azimuths.sort_unstable_by(f64::total_cmp);
    let (Some(first), Some(last)) = (azimuths.first(), azimuths.last()) else {
        return 360.0;
    };
    azimuths
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .fold(first + 360.0 - last, f64::max)
}

fn conditioning(source: DVec2, data: &[Datum]) -> f64 {
    // Center the time derivatives to remove the common origin-time degree of freedom.
    let gradient = |datum: &Datum| {
        let delta = source - datum.station;
        let range = delta.length().hypot(datum.depth_to_station);
        delta / (datum.velocity * range.max(0.001))
    };
    let mean = data.iter().map(gradient).sum::<DVec2>() / data.len() as f64;
    let moments = data
        .iter()
        .map(|datum| {
            let g = gradient(datum) - mean;
            DVec3::new(g.x * g.x, g.y * g.y, g.x * g.y)
        })
        .sum::<DVec3>();
    let determinant = moments.x * moments.y - moments.z * moments.z;
    determinant / (moments.x + moments.y).powi(2).max(f64::EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station_replay::{Pick, Station, StationId};

    fn fixture() -> (Origin, Vec<Observation>, GeoPoint) {
        let point = GeoPoint {
            lat: 45.0,
            lon: 179.96,
        };
        let origin = Origin {
            point,
            time_ms: 1_000_000,
            depth_km: 8.0,
        };
        let frame = LocalFrame::new(point);
        let expected = DVec2::new(13.0, -6.0);
        let observations = [
            (30., 22.),
            (-22., 25.),
            (-29., -17.),
            (37., -26.),
            (10., 50.),
            (50., 1.),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (x, y))| {
            let location = DVec2::new(x, y);
            let id = StationId {
                network: "XX".into(),
                code: format!("S{i}"),
            };
            let phase = if i == 3 { "Sg" } else { "Pg" };
            let velocity = if i == 3 { 3.5 } else { 6.0 };
            let outlier = if i == 5 { 29.0 } else { 0.0 };
            let seconds = 1.75 + location.distance(expected).hypot(8.5) / velocity + outlier;
            Observation {
                station: id,
                location: Station {
                    point: frame.to_geo(location),
                    elevation_km: 0.5,
                },
                picks: vec![Pick {
                    phase: phase.into(),
                    time_ms: origin.time_ms + (seconds * 1000.0).round() as i64,
                    used: true,
                }],
            }
        })
        .collect();
        (origin, observations, frame.to_geo(expected))
    }

    #[test]
    fn refits_coordinates_and_origin_time_across_antimeridian_from_independent_stations() {
        let (origin, observations, expected) = fixture();
        let answer = locate(&origin, &observations).unwrap();
        assert!(distance_km(answer.point, expected) < 0.25);
        assert!((answer.time_offset_s - 1.75).abs() < 0.08);
        assert_eq!(answer.stations, 5);
        assert!(answer.rms_s < 0.05);
    }

    #[test]
    fn refuses_fit_when_stations_do_not_surround_the_source() {
        let (origin, mut observations, _) = fixture();
        let frame = LocalFrame::new(origin.point);
        observations.pop();
        for (i, station) in observations.iter_mut().enumerate() {
            let east = 75.0 + i as f64 * 10.0;
            station.location.point = frame.to_geo(DVec2::new(east, 0.0));
            station.picks[0].phase = "P".into();
            station.picks[0].time_ms =
                origin.time_ms + (east.hypot(8.5) / 6.0 * 1000.0).round() as i64;
        }
        assert!(locate(&origin, &observations).is_err());
    }
}
