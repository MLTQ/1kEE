/// Planetary position computation for the stellar correspondence layer.
///
/// # Method
/// Keplerian orbital elements from the JPL "Keplerian Elements for Approximate
/// Planetary Positions" dataset (Table 1 — valid 3000 BC to 3000 AD).
/// Secular rates convert mean elements to the target epoch, then a standard
/// Keplerian orbit solve gives heliocentric ecliptic coordinates.  Subtracting
/// Earth's heliocentric position gives geocentric ecliptic, which is rotated to
/// equatorial via the mean obliquity.
///
/// # Accuracy
/// ≈ 1–5 arc-minutes for most planets within the 3000 BC–3000 AD window.
/// Degrades to ≈ 1–5° at extreme epochs (Göbekli Tepe, Lascaux) where first-
/// order secular terms diverge.  Adequate for a visual globe layer.
///
/// Moon uses the simplified Meeus ELP2000 single-term approximation (≈ 1–2°).

use crate::stellar_time;

// ── Planet list ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Planet {
    Sun,
    Moon,
    Mercury,
    Venus,
    Mars,
    Jupiter,
    Saturn,
    Uranus,
    Neptune,
}

impl Planet {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sun     => "Sun",
            Self::Moon    => "Moon",
            Self::Mercury => "Mercury",
            Self::Venus   => "Venus",
            Self::Mars    => "Mars",
            Self::Jupiter => "Jupiter",
            Self::Saturn  => "Saturn",
            Self::Uranus  => "Uranus",
            Self::Neptune => "Neptune",
        }
    }

    /// Display color for this body.
    pub fn color(self) -> egui::Color32 {
        match self {
            Self::Sun     => egui::Color32::from_rgb(255, 230,  80), // gold
            Self::Moon    => egui::Color32::from_rgb(210, 215, 225), // silver
            Self::Mercury => egui::Color32::from_rgb(180, 175, 175), // grey
            Self::Venus   => egui::Color32::from_rgb(255, 215, 110), // cream-yellow
            Self::Mars    => egui::Color32::from_rgb(225,  80,  50), // rust red
            Self::Jupiter => egui::Color32::from_rgb(255, 175,  90), // amber-orange
            Self::Saturn  => egui::Color32::from_rgb(215, 195, 120), // pale gold
            Self::Uranus  => egui::Color32::from_rgb(115, 205, 230), // ice blue
            Self::Neptune => egui::Color32::from_rgb( 75, 115, 215), // deep blue
        }
    }

    /// Dot radius for this body on the globe (screen pixels).
    pub fn radius(self) -> f32 {
        match self {
            Self::Sun     => 7.0,
            Self::Moon    => 5.5,
            Self::Mercury => 3.0,
            Self::Venus   => 4.5,
            Self::Mars    => 4.0,
            Self::Jupiter => 5.5,
            Self::Saturn  => 5.0,
            Self::Uranus  => 3.5,
            Self::Neptune => 3.5,
        }
    }

    /// Recommended trail span in days to show interesting motion / retrograde loops.
    pub fn default_trail_days(self) -> f64 {
        match self {
            Self::Sun     =>   365.25,
            Self::Moon    =>    29.53 * 3.0, // 3 lunar months
            Self::Mercury =>   365.25,        // several retrograde loops
            Self::Venus   =>   365.25 * 1.5,
            Self::Mars    =>   365.25 * 2.0,  // one retrograde arc
            Self::Jupiter =>   365.25 * 5.0,
            Self::Saturn  =>   365.25 * 10.0,
            Self::Uranus  =>   365.25 * 20.0,
            Self::Neptune =>   365.25 * 40.0,
        }
    }
}

pub const ALL_PLANETS: &[Planet] = &[
    Planet::Sun, Planet::Moon, Planet::Mercury, Planet::Venus,
    Planet::Mars, Planet::Jupiter, Planet::Saturn, Planet::Uranus, Planet::Neptune,
];

// ── Orbital elements ──────────────────────────────────────────────────────────
// Source: JPL "Keplerian Elements for Approximate Planetary Positions" Table 1
// (3000 BC – 3000 AD).  T = Julian centuries from J2000.0.
// Quantities: semi-major axis a (AU), eccentricity e (dimensionless),
//   inclination I (°), mean longitude L (°), longitude of perihelion ω̄ (°),
//   longitude of ascending node Ω (°); each with a secular rate per century.

struct OrbElements {
    a0: f64, adot: f64,
    e0: f64, edot: f64,
    i0: f64, idot: f64,
    l0: f64, ldot: f64,
    lp0: f64, lpdot: f64, // ω̄ = Ω + ω
    ln0: f64, lndot: f64, // Ω
}

impl OrbElements {
    /// Evaluate elements at T Julian centuries from J2000.
    #[inline]
    fn at(&self, t: f64) -> (f64, f64, f64, f64, f64, f64) {
        (
            self.a0  + self.adot  * t,
            (self.e0 + self.edot  * t).clamp(0.0, 0.999),
            self.i0  + self.idot  * t,
            self.l0  + self.ldot  * t,
            self.lp0 + self.lpdot * t,
            self.ln0 + self.lndot * t,
        )
    }
}

const MERCURY: OrbElements = OrbElements {
    a0: 0.387_098_43, adot:  0.000_000_00,
    e0: 0.205_636_61, edot:  0.000_020_23,
    i0: 7.005_594_32, idot: -0.005_901_58,
    l0: 252.251_667_24, ldot: 149_472.674_866_23,
    lp0:  77.457_718_95, lpdot: 0.159_400_13,
    ln0:  48.339_618_19, lndot: -0.122_141_82,
};

const VENUS: OrbElements = OrbElements {
    a0: 0.723_321_02, adot: -0.000_000_26,
    e0: 0.006_763_99, edot: -0.000_051_07,
    i0: 3.397_775_45, idot:  0.000_434_94,
    l0: 181.979_708_50, ldot: 58_517.815_602_60,
    lp0: 131.767_557_13, lpdot: 0.056_796_48,
    ln0:  76.672_614_96, lndot: -0.272_741_74,
};

const EARTH: OrbElements = OrbElements {
    a0: 1.000_000_18, adot: -0.000_000_03,
    e0: 0.016_731_63, edot: -0.000_036_61,
    i0: -0.000_543_46, idot: -0.013_371_78,
    l0: 100.464_571_66, ldot: 35_999.372_449_81,
    lp0: 102.937_681_93, lpdot: 0.323_273_64,
    ln0:  -5.112_603_89, lndot: -0.241_233_53,
};

const MARS: OrbElements = OrbElements {
    a0: 1.523_712_43, adot:  0.000_000_97,
    e0: 0.093_365_11, edot:  0.000_091_49,
    i0: 1.851_818_69, idot: -0.007_247_57,
    l0: -4.568_131_64, ldot: 19_140.299_342_43,
    lp0: -23.917_447_84, lpdot: 0.452_236_25,
    ln0:  49.713_209_84, lndot: -0.268_524_31,
};

const JUPITER: OrbElements = OrbElements {
    a0:  5.202_480_19, adot: -0.000_028_64,
    e0:  0.048_535_90, edot:  0.000_180_26,
    i0:  1.298_614_16, idot: -0.003_226_99,
    l0: 34.334_791_52, ldot: 3_034.903_717_57,
    lp0: 14.274_952_44, lpdot: 0.181_991_96,
    ln0: 100.292_826_54, lndot: 0.130_246_19,
};

const SATURN: OrbElements = OrbElements {
    a0:  9.541_498_83, adot: -0.000_030_65,
    e0:  0.055_508_25, edot: -0.000_320_44,
    i0:  2.494_241_02, idot:  0.004_519_69,
    l0: 50.075_713_29, ldot: 1_222.114_947_24,
    lp0: 92.861_360_63, lpdot: 0.541_794_78,
    ln0: 113.639_987_02, lndot: -0.250_150_02,
};

const URANUS: OrbElements = OrbElements {
    a0: 19.187_979_48, adot: -0.000_204_55,
    e0:  0.046_857_40, edot: -0.000_015_50,
    i0:  0.772_981_27, idot: -0.001_801_55,
    l0: 314.202_766_25, ldot: 428.495_125_95,
    lp0: 172.434_044_41, lpdot: 0.092_669_85,
    ln0:  73.962_502_15, lndot:  0.057_396_99,
};

const NEPTUNE: OrbElements = OrbElements {
    a0: 30.069_527_52, adot:  0.000_064_47,
    e0:  0.008_954_39, edot:  0.000_008_18,
    i0:  1.770_055_20, idot:  0.000_224_00,
    l0: 304.222_892_87, ldot: 218.465_153_14,
    lp0: 46.681_587_24, lpdot: 0.010_099_38,
    ln0: 131.786_358_53, lndot: -0.006_063_02,
};

// ── Kepler & coordinate helpers ───────────────────────────────────────────────

/// Solve Kepler's equation `E − e·sin(E) = M` for eccentric anomaly E (radians).
fn kepler(m_deg: f64, e: f64) -> f64 {
    let m = m_deg.to_radians();
    let mut ea = m;
    for _ in 0..50 {
        let delta = (m - ea + e * ea.sin()) / (1.0 - e * ea.cos());
        ea += delta;
        if delta.abs() < 1e-12 {
            break;
        }
    }
    ea
}

/// Keplerian elements → heliocentric ecliptic rectangular coordinates (AU).
fn keplerian_to_ecliptic(a: f64, e: f64, i_deg: f64, l_deg: f64, lp_deg: f64, ln_deg: f64) -> (f64, f64, f64) {
    let omega = (lp_deg - ln_deg).to_radians(); // argument of perihelion
    let m_deg = (l_deg - lp_deg).rem_euclid(360.0);
    let ea    = kepler(m_deg, e);

    // True anomaly
    let nu = 2.0 * (((1.0 + e) / (1.0 - e)).sqrt() * (ea * 0.5).tan()).atan();
    let r  = a * (1.0 - e * ea.cos());

    let i  = i_deg.to_radians();
    let ln = ln_deg.to_radians();
    let u  = nu + omega;

    let x = r * (ln.cos() * u.cos() - ln.sin() * u.sin() * i.cos());
    let y = r * (ln.sin() * u.cos() + ln.cos() * u.sin() * i.cos());
    let z = r * u.sin() * i.sin();
    (x, y, z)
}

/// Rotate ecliptic rectangular → equatorial rectangular via obliquity ε.
#[inline]
fn ecl_to_eq(x: f64, y: f64, z: f64, eps_deg: f64) -> (f64, f64, f64) {
    let eps = eps_deg.to_radians();
    (x, eps.cos() * y - eps.sin() * z, eps.sin() * y + eps.cos() * z)
}

/// Equatorial rectangular → (RA degrees, Dec degrees).
#[inline]
fn xyz_to_radec(x: f64, y: f64, z: f64) -> (f64, f64) {
    let ra  = y.atan2(x).to_degrees().rem_euclid(360.0);
    let dec = z.atan2((x * x + y * y).sqrt()).to_degrees();
    (ra, dec)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Compute geocentric equatorial (RA, Dec) in degrees for the given body at JD.
///
/// Returns `None` only for internal match exhaustion (shouldn't happen).
pub fn geocentric_radec(planet: Planet, jd: f64) -> Option<(f64, f64)> {
    let t   = stellar_time::j2000_centuries(jd);
    let eps = stellar_time::obliquity_deg(jd);

    // ── Earth's heliocentric ecliptic position ──────────────────────────────
    let (ae, ee, ie, le, lpe, lne) = EARTH.at(t);
    let (xe, ye, ze) = keplerian_to_ecliptic(ae, ee, ie, le, lpe, lne);

    match planet {
        Planet::Sun => {
            // Geocentric Sun = negated Earth heliocentric vector
            let (x, y, z) = ecl_to_eq(-xe, -ye, -ze, eps);
            Some(xyz_to_radec(x, y, z))
        }

        Planet::Moon => {
            // Simplified ELP2000 (Meeus Ch. 47, first-term approximation).
            // Accuracy ≈ 1–2°; adequate for a visual layer.
            let d = jd - 2_451_545.0;
            let m  = (134.963 + 13.064_993 * d).to_radians(); // Moon's mean anomaly
            let f  = ( 93.272 + 13.229_350 * d).to_radians(); // argument of latitude
            let ld = (218.316 + 13.176_396 * d) + 6.289 * m.sin();
            let bd = 5.128 * f.sin();
            let lr = ld.to_radians();
            let br = bd.to_radians();
            let (x, y, z) = ecl_to_eq(lr.cos() * br.cos(), lr.sin() * br.cos(), br.sin(), eps);
            Some(xyz_to_radec(x, y, z))
        }

        other => {
            let elems: &OrbElements = match other {
                Planet::Mercury => &MERCURY,
                Planet::Venus   => &VENUS,
                Planet::Mars    => &MARS,
                Planet::Jupiter => &JUPITER,
                Planet::Saturn  => &SATURN,
                Planet::Uranus  => &URANUS,
                Planet::Neptune => &NEPTUNE,
                _               => return None,
            };
            let (ap, ep, ip, lp, lpp, lnp) = elems.at(t);
            let (xp, yp, zp) = keplerian_to_ecliptic(ap, ep, ip, lp, lpp, lnp);
            let (x, y, z) = ecl_to_eq(xp - xe, yp - ye, zp - ze, eps);
            Some(xyz_to_radec(x, y, z))
        }
    }
}

/// Compute a geographic trail: a list of `(lon, lat)` pairs sampled at
/// `n_points` evenly over `span_days` centred on `center_jd`.
///
/// Longitude wraps within \[−180, 180\].
pub fn planet_trail(
    planet: Planet,
    center_jd: f64,
    span_days: f64,
    n_points: usize,
) -> Vec<(f32, f32)> {
    let start = center_jd - span_days * 0.5;
    let step  = span_days / (n_points.saturating_sub(1).max(1)) as f64;
    let mut trail = Vec::with_capacity(n_points);

    for i in 0..n_points {
        let jd   = start + step * i as f64;
        let gmst = stellar_time::gmst_deg(jd);
        if let Some((ra, dec)) = geocentric_radec(planet, jd) {
            let lon_raw = (ra - gmst).rem_euclid(360.0);
            let lon     = if lon_raw > 180.0 { lon_raw - 360.0 } else { lon_raw };
            trail.push((lon as f32, dec as f32));
        }
    }
    trail
}
