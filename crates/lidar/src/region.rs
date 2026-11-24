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
        if !lon.is_finite() || !lat.is_finite() {
            return false;
        }
        // Strict Riemannian bounds on WGS84: every geodesic arc length s
        // relative to unit-sphere central angle theta satisfies
        // M_min * theta <= s <= N_max * theta, where M_min = 6_335_439 m
        // and N_max = 6_399_594 m. Fast-accept interior points and fast-reject
        // exterior points without running iterative Vincenty, falling back to
        // exact Vincenty only inside the ~1% boundary annulus.
        const MIN_R: f64 = 6_330_000.;
        const MAX_R: f64 = 6_405_000.;
        let dlat = (lat - self.lat).to_radians();
        if dlat.abs() * MIN_R > self.radius_m + 1. {
            return false;
        }
        let dlon = (lon - self.lon).to_radians();
        let sin_dlat = (dlat * 0.5).sin();
        let sin_dlon = (dlon * 0.5).sin();
        let h = sin_dlat * sin_dlat
            + self.lat.to_radians().cos() * lat.to_radians().cos() * sin_dlon * sin_dlon;
        let max_half = (self.radius_m + 1e-6) / (2. * MIN_R);
        if h > max_half * max_half {
            return false;
        }
        let min_half = (self.radius_m - 1e-6).max(0.) / (2. * MAX_R);
        if h < min_half * min_half {
            return true;
        }
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
fn delta_sigma_sincos(b: f64, sin_sigma: f64, cos_sigma: f64, cos_two: f64) -> f64 {
    let cos_two_sq = cos_two * cos_two;
    b * sin_sigma
        * (cos_two
            + b / 4.
                * (cos_sigma * (-1. + 2. * cos_two_sq)
                    - b / 6.
                        * cos_two
                        * (-3. + 4. * sin_sigma * sin_sigma)
                        * (-3. + 4. * cos_two_sq)))
}
fn delta_sigma(b: f64, sigma: f64, cos_two: f64) -> f64 {
    let (sin_sigma, cos_sigma) = sigma.sin_cos();
    delta_sigma_sincos(b, sin_sigma, cos_sigma, cos_two)
}
fn inverse(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> (f64, f64) {
    let u1 = ((1. - F) * lat1.to_radians().tan()).atan();
    let u2 = ((1. - F) * lat2.to_radians().tan()).atan();
    let (sin_u1, cos_u1) = u1.sin_cos();
    let (sin_u2, cos_u2) = u2.sin_cos();
    let l = ((lon2 - lon1 + 180.).rem_euclid(360.) - 180.).to_radians();
    let mut lambda = l;
    let mut sin_sigma = 0.;
    let mut cos_sigma = 0.;
    let mut sigma = 0.;
    let mut cos_sq_alpha = 0.;
    let mut cos_two = 0.;
    let mut sin_lam = 0.;
    let mut cos_lam = 0.;
    for _ in 0..30 {
        (sin_lam, cos_lam) = lambda.sin_cos();
        let t1 = cos_u2 * sin_lam;
        let t2 = cos_u1 * sin_u2 - sin_u1 * cos_u2 * cos_lam;
        sin_sigma = t1.hypot(t2);
        if sin_sigma == 0. {
            return (0., 0.);
        }
        cos_sigma = sin_u1 * sin_u2 + cos_u1 * cos_u2 * cos_lam;
        sigma = sin_sigma.atan2(cos_sigma);
        let sin_alpha = cos_u1 * cos_u2 * sin_lam / sin_sigma;
        cos_sq_alpha = (1. - sin_alpha * sin_alpha).max(0.);
        cos_two = if cos_sq_alpha < 1e-20 {
            0.
        } else {
            cos_sigma - 2. * sin_u1 * sin_u2 / cos_sq_alpha
        };
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
    let (a, b) = coefficients(cos_sq_alpha);
    let distance = B * a * (sigma - delta_sigma_sincos(b, sin_sigma, cos_sigma, cos_two));
    let bearing = (cos_u2 * sin_lam).atan2(cos_u1 * sin_u2 - sin_u1 * cos_u2 * cos_lam);
    (distance, bearing)
}
fn direct(lon: f64, lat: f64, bearing: f64, distance: f64) -> [f64; 2] {
    let u = ((1. - F) * lat.to_radians().tan()).atan();
    let (sin_u, cos_u) = u.sin_cos();
    let (sin_b, cos_b) = bearing.sin_cos();
    let sigma1 = (sin_u / cos_u).atan2(cos_b);
    let sin_alpha = cos_u * sin_b;
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
    let (sin_sigma, cos_sigma) = sigma.sin_cos();
    let temp = sin_u * sin_sigma - cos_u * cos_sigma * cos_b;
    let lat2 = (sin_u * cos_sigma + cos_u * sin_sigma * cos_b)
        .atan2((1. - F) * (sin_alpha * sin_alpha + temp * temp).sqrt());
    let lambda = (sin_sigma * sin_b).atan2(cos_u * cos_sigma - sin_u * sin_sigma * cos_b);
    let c = F / 16. * cos_sq_alpha * (4. + F * (4. - 3. * cos_sq_alpha));
    let cos_two = (2. * sigma1 + sigma).cos();
    let l = lambda
        - (1. - c)
            * F
            * sin_alpha
            * (sigma
                + c * sin_sigma * (cos_two + c * cos_sigma * (-1. + 2. * cos_two * cos_two)));
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
    #[test]
    fn fast_contains_matches_exact_vincenty_across_latitudes_and_radii() {
        for (lon, lat) in [(0., 0.), (-77.58, 35.26), (179.99, 55.), (-100., 89.)] {
            for radius_m in [10., 1_000., 100_000.] {
                let r = Region { lon, lat, radius_m };
                for scale in [0.5, 0.98, 0.995, 0.99999, 1.00001, 1.005, 1.02, 1.5] {
                    for angle_idx in 0..24 {
                        let a = angle_idx as f64 * std::f64::consts::TAU / 24.;
                        let p = r.location(a.sin() * radius_m * scale, a.cos() * radius_m * scale);
                        let [x, y] = r.offset(p[0], p[1]);
                        let exact = x.is_finite() && y.is_finite() && x.hypot(y) <= radius_m + 1e-6;
                        assert_eq!(
                            r.contains(p[0], p[1]),
                            exact,
                            "mismatch for {r:?} scale={scale} angle={angle_idx}"
                        );
                    }
                }
            }
        }
    }
}
