//! WGS84 radial selection and metre coordinates, calculated in float64.
const A: f64 = 6_378_137.;
const F: f64 = 1. / 298.257_223_563;
const B: f64 = A * (1. - F);
pub const MAX_RADIUS_M: f64 = 100_000.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub lon: f64,
    pub lat: f64,
    pub radius_m: f64,
}

impl Region {
    pub fn valid(self) -> bool {
        self.lon.is_finite()
            && (-180. ..=180.).contains(&self.lon)
            && self.lat.is_finite()
            && (-89. ..=89.).contains(&self.lat)
            && self.radius_m.is_finite()
            && (1. ..=MAX_RADIUS_M).contains(&self.radius_m)
    }

    /// East/north geodesic distance on the ellipsoid; Z is handled separately
    /// in the source vertical reference, never interpreted as ellipsoid height.
    pub fn offset(self, lon: f64, lat: f64) -> [f64; 2] {
        let (distance, bearing) = inverse(self.lon, self.lat, lon, lat);
        [distance * bearing.sin(), distance * bearing.cos()]
    }

    pub fn contains(self, lon: f64, lat: f64) -> bool {
        let [x, y] = self.offset(lon, lat);
        x.is_finite() && y.is_finite() && x.hypot(y) <= self.radius_m + 1e-6
    }

    pub fn location(self, east: f64, north: f64) -> [f64; 2] {
        direct(self.lon, self.lat, east.atan2(north), east.hypot(north))
    }

    /// Longitude is unwrapped around the center. Consumers split date-line
    /// windows where their source API requires longitudes within ±180°.
    pub fn bounds(self) -> [f64; 4] {
        let mut b = [self.lon, self.lat, self.lon, self.lat];
        for i in 0..256 {
            let angle = i as f64 * std::f64::consts::TAU / 256.;
            let [mut lon, lat] = self.location(
                angle.sin() * self.radius_m * 1.001,
                angle.cos() * self.radius_m * 1.001,
            );
            lon = self.lon + (lon - self.lon + 180.).rem_euclid(360.) - 180.;
            b = [b[0].min(lon), b[1].min(lat), b[2].max(lon), b[3].max(lat)];
        }
        b
    }
}

// Vincenty's direct/inverse formulae. Target windows are at most 200 km wide,
// far from the antipodal convergence singularity.
fn coefficients(cos_sq_alpha: f64) -> (f64, f64) {
    let u = cos_sq_alpha * (A * A - B * B) / (B * B);
    (
        1. + u / 16384. * (4096. + u * (-768. + u * (320. - 175. * u))),
        u / 1024. * (256. + u * (-128. + u * (74. - 47. * u))),
    )
}
fn delta_sigma(b: f64, sigma: f64, cos_two: f64) -> f64 {
    b * sigma.sin()
        * (cos_two
            + b / 4.
                * (sigma.cos() * (-1. + 2. * cos_two.powi(2))
                    - b / 6.
                        * cos_two
                        * (-3. + 4. * sigma.sin().powi(2))
                        * (-3. + 4. * cos_two.powi(2))))
}
fn inverse(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> (f64, f64) {
    let u1 = ((1. - F) * lat1.to_radians().tan()).atan();
    let u2 = ((1. - F) * lat2.to_radians().tan()).atan();
    let l = ((lon2 - lon1 + 180.).rem_euclid(360.) - 180.).to_radians();
    let mut lambda = l;
    let mut result = (0., 0.);
    for _ in 0..30 {
        let sin_sigma = (u2.cos() * lambda.sin())
            .hypot(u1.cos() * u2.sin() - u1.sin() * u2.cos() * lambda.cos());
        if sin_sigma == 0. {
            return (0., 0.);
        }
        let cos_sigma = u1.sin() * u2.sin() + u1.cos() * u2.cos() * lambda.cos();
        let sigma = sin_sigma.atan2(cos_sigma);
        let sin_alpha = u1.cos() * u2.cos() * lambda.sin() / sin_sigma;
        let cos_sq_alpha = (1. - sin_alpha * sin_alpha).max(0.);
        let cos_two = if cos_sq_alpha < 1e-20 {
            0.
        } else {
            cos_sigma - 2. * u1.sin() * u2.sin() / cos_sq_alpha
        };
        let (a, b) = coefficients(cos_sq_alpha);
        let distance = B * a * (sigma - delta_sigma(b, sigma, cos_two));
        let bearing = (u2.cos() * lambda.sin())
            .atan2(u1.cos() * u2.sin() - u1.sin() * u2.cos() * lambda.cos());
        result = (distance, bearing);
        let c = F / 16. * cos_sq_alpha * (4. + F * (4. - 3. * cos_sq_alpha));
        let next = l
            + (1. - c)
                * F
                * sin_alpha
                * (sigma
                    + c * sin_sigma * (cos_two + c * cos_sigma * (-1. + 2. * cos_two * cos_two)));
        if (next - lambda).abs() < 1e-13 {
            break;
        }
        lambda = next;
    }
    result
}
fn direct(lon: f64, lat: f64, bearing: f64, distance: f64) -> [f64; 2] {
    let u = ((1. - F) * lat.to_radians().tan()).atan();
    let sigma1 = u.tan().atan2(bearing.cos());
    let sin_alpha = u.cos() * bearing.sin();
    let cos_sq_alpha = 1. - sin_alpha * sin_alpha;
    let (a, b) = coefficients(cos_sq_alpha);
    let mut sigma = distance / (B * a);
    for _ in 0..20 {
        let next = distance / (B * a) + delta_sigma(b, sigma, (2. * sigma1 + sigma).cos());
        if (next - sigma).abs() < 1e-13 {
            break;
        }
        sigma = next;
    }
    let temp = u.sin() * sigma.sin() - u.cos() * sigma.cos() * bearing.cos();
    let lat2 = (u.sin() * sigma.cos() + u.cos() * sigma.sin() * bearing.cos())
        .atan2((1. - F) * (sin_alpha * sin_alpha + temp * temp).sqrt());
    let lambda = (sigma.sin() * bearing.sin())
        .atan2(u.cos() * sigma.cos() - u.sin() * sigma.sin() * bearing.cos());
    let c = F / 16. * cos_sq_alpha * (4. + F * (4. - 3. * cos_sq_alpha));
    let cos_two = (2. * sigma1 + sigma).cos();
    let l = lambda
        - (1. - c)
            * F
            * sin_alpha
            * (sigma
                + c * sigma.sin() * (cos_two + c * sigma.cos() * (-1. + 2. * cos_two * cos_two)));
    [
        (lon + l.to_degrees() + 180.).rem_euclid(360.) - 180.,
        lat2.to_degrees(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maximum_target_radius_bounds_enclose_the_full_wgs84_circle() {
        for (lon, lat) in [(0., 0.), (-77.58, 35.26), (179.99, 55.), (-100., 89.)] {
            for radius_m in [100., 1000., 100_000.] {
                let r = Region { lon, lat, radius_m };
                assert!(r.valid());
                let b = r.bounds();
                for i in 0..720 {
                    let angle = i as f64 * std::f64::consts::TAU / 720.;
                    let xy = [angle.sin() * radius_m, angle.cos() * radius_m];
                    let p = r.location(xy[0], xy[1]);
                    let q = r.offset(p[0], p[1]);
                    assert!((q[0] - xy[0]).hypot(q[1] - xy[1]) < 0.0001);
                    let x = lon + (p[0] - lon + 180.).rem_euclid(360.) - 180.;
                    assert!(
                        x >= b[0] && x <= b[2] && p[1] >= b[1] && p[1] <= b[3],
                        "{r:?} {b:?} {p:?}"
                    );
                }
            }
        }
        assert!(
            !Region {
                lon: 0.,
                lat: 0.,
                radius_m: 100_001.
            }
            .valid()
        );
    }
    #[test]
    fn wgs84_radius_roundtrips_at_equator_nc_poles_and_date_line() {
        let equator = Region {
            lon: 0.,
            lat: 0.,
            radius_m: 1000.,
        };
        assert!((equator.offset(1., 0.)[0] - 111_319.490_793).abs() < 1e-5);
        for (lon, lat) in [(0., 0.), (-77.58, 35.26), (179.999, 55.), (-100., 88.)] {
            let r = Region {
                lon,
                lat,
                radius_m: 1000.,
            };
            for angle in 0..36 {
                let a = angle as f64 * std::f64::consts::TAU / 36.;
                let xy = [a.sin() * 1000., a.cos() * 1000.];
                let p = r.location(xy[0], xy[1]);
                let q = r.offset(p[0], p[1]);
                assert!((q[0] - xy[0]).hypot(q[1] - xy[1]) < 1e-5, "{r:?} {q:?}");
                assert!(r.contains(p[0], p[1]));
                let outside = r.location(xy[0] * 1.0001, xy[1] * 1.0001);
                assert!(!r.contains(outside[0], outside[1]));
            }
        }
    }
}
