/// Time utilities for the stellar correspondence layer.
///
/// The central concept is the **Julian Date** (JD): a continuous day count since
/// noon, 1 January 4713 BCE (proleptic Julian calendar).  It is the standard
/// time axis for astronomical calculations.
///
/// To project a star's fixed equatorial position (RA, Dec) onto Earth's surface
/// at a given moment we subtract the **Greenwich Mean Sidereal Time** (GMST) from
/// the Right Ascension.  The result is the **Geographic Position** (GP) — the
/// point on Earth directly below the star at that instant.

use std::f64::consts::PI;

/// Julian Date of the Unix epoch (1970-01-01 00:00:00 UTC).
pub const JD_UNIX: f64 = 2_440_587.5;

/// Julian Date of the J2000.0 standard epoch (2000-01-01 12:00 TT).
pub const JD_J2000: f64 = 2_451_545.0;

// ── Historical epoch presets (approximate JD) ─────────────────────────────────
pub mod epoch {
    /// J2000.0 standard reference epoch (2000-01-01 12:00 TT).
    pub const J2000: f64 = 2_451_545.0;
    /// Apollo 11 lunar landing: 1969-07-20 20:17 UTC.
    pub const APOLLO_11: f64 = 2_440_423.346;
    /// Trinity nuclear test: 1945-07-16 05:29 UTC.
    pub const TRINITY: f64 = 2_431_244.729;
    /// Fall of Rome (conventional end): 476 CE.
    pub const FALL_OF_ROME: f64 = 1_869_720.5;
    /// Construction era of the Great Pyramid at Giza: ~2560 BCE.
    pub const GIZA_PYRAMIDS: f64 = 785_798.5;
    /// Construction of the Great Sphinx: ~2500 BCE.
    pub const SPHINX: f64 = 807_730.5;
    /// Göbekli Tepe construction: ~9600 BCE.
    pub const GOBEKLI_TEPE: f64 = -1_784_747.5;
    /// Lascaux cave paintings: ~17 000 BCE.
    pub const LASCAUX: f64 = -4_736_747.5;
}

// ── Basic conversions ─────────────────────────────────────────────────────────

/// Unix timestamp (seconds since 1970-01-01 00:00 UTC) → Julian Date.
pub fn unix_to_jd(unix_secs: f64) -> f64 {
    unix_secs / 86_400.0 + JD_UNIX
}

/// Julian Date → Unix timestamp.
pub fn jd_to_unix(jd: f64) -> f64 {
    (jd - JD_UNIX) * 86_400.0
}

/// Julian centuries from J2000.0 for the given Julian Date.
pub fn j2000_centuries(jd: f64) -> f64 {
    (jd - JD_J2000) / 36_525.0
}

// ── Sidereal time ─────────────────────────────────────────────────────────────

/// Greenwich Mean Sidereal Time in degrees \[0, 360).
///
/// IAU 1982 formula.  Accurate to ≈ 0.1″ from 1900–2100; degrades gracefully
/// to ≈ minutes-of-arc over millennia (higher-order terms in Earth's rotation
/// rate are not modelled, but the dominant 360°/sidereal-day term is exact).
pub fn gmst_deg(jd: f64) -> f64 {
    let t = j2000_centuries(jd);
    let du = jd - JD_J2000; // fractional days since J2000
    let gmst = 280.460_618_37
        + 360.985_647_366_29 * du
        + 0.000_387_933 * t * t
        - t * t * t / 38_710_000.0;
    gmst.rem_euclid(360.0)
}

// ── Obliquity ─────────────────────────────────────────────────────────────────

/// Mean obliquity of the ecliptic in degrees.
///
/// Laskar 1986 formula, valid to 0.01″ over 1000 years and a few arc-minutes
/// over 10 000 years.
pub fn obliquity_deg(jd: f64) -> f64 {
    let t = j2000_centuries(jd);
    23.439_291_111
        - 0.013_004_167 * t
        - 1.64e-7 * t * t
        + 5.04e-7 * t * t * t
}

// ── Precession ────────────────────────────────────────────────────────────────

/// Precess equatorial J2000.0 coordinates (ra_deg, dec_deg) to the frame of
/// the given Julian Date, using the IAU 1976 precession model.
///
/// Accuracy: ≈ 3″ over 1 000 years; degrades to ≈ 1° over 10 000 years.
/// Sufficient for visual display at any epoch humans have occupied Earth.
///
/// Returns `(ra_deg, dec_deg)` in the precessed frame.
pub fn precess_j2000(ra_deg: f64, dec_deg: f64, jd: f64) -> (f64, f64) {
    let t = j2000_centuries(jd);
    let arcsec_to_rad = PI / (180.0 * 3_600.0);

    // IAU 1976 polynomial coefficients in arcseconds
    let zeta  = (2306.2181 + 1.39656 * t - 0.000_139 * t * t) * t;
    let z     = (2306.2181 + 1.09468 * t + 0.000_066 * t * t) * t;
    let theta = (2004.3109 - 0.853_30 * t - 0.000_217 * t * t) * t;

    let zeta_r  = zeta  * arcsec_to_rad;
    let z_r     = z     * arcsec_to_rad;
    let theta_r = theta * arcsec_to_rad;

    let ra  = ra_deg.to_radians();
    let dec = dec_deg.to_radians();

    // Standard precession rotation (Meeus Ch. 21)
    let a = dec.cos() * (ra + zeta_r).sin();
    let b = theta_r.cos() * dec.cos() * (ra + zeta_r).cos()
          - theta_r.sin() * dec.sin();
    let c = theta_r.sin() * dec.cos() * (ra + zeta_r).cos()
          + theta_r.cos() * dec.sin();

    let ra_new  = (a.atan2(b) + z_r).to_degrees().rem_euclid(360.0);
    let dec_new = c.asin().to_degrees();
    (ra_new, dec_new)
}

// ── Calendar display ──────────────────────────────────────────────────────────

/// Convert a Julian Date to a human-readable calendar string.
///
/// Uses the proleptic Gregorian calendar (Meeus Ch. 7 algorithm).
/// Returns strings like `"2026-03-30 14:22:00"` for CE dates and
/// `"9600-01-12 00:00:00 BCE"` for BCE dates.
pub fn jd_to_string(jd: f64) -> String {
    // Guard extreme dates — algorithm degrades and we just want an approx year
    let t_centuries = j2000_centuries(jd);
    if t_centuries.abs() > 500.0 {
        let years = (jd - JD_J2000) / 365.25;
        let year = (2000.0 + years).round() as i64;
        return if year <= 0 {
            format!("~{} BCE", 1 - year)
        } else {
            format!("~{} CE", year)
        };
    }

    let jd_shifted = jd + 0.5;
    let z = jd_shifted.floor() as i64;
    let f = jd_shifted - z as f64; // fractional day

    let a = if z < 2_299_161 {
        z
    } else {
        let alpha = ((z as f64 - 1_867_216.25) / 36_524.25).floor() as i64;
        z + 1 + alpha - alpha / 4
    };
    let b = a + 1524;
    let c = ((b as f64 - 122.1) / 365.25).floor() as i64;
    let d = (365.25 * c as f64).floor() as i64;
    let e = ((b - d) as f64 / 30.6001).floor() as i64;

    let day   = b - d - (30.6001 * e as f64).floor() as i64;
    let month = if e < 14 { e - 1 } else { e - 13 };
    let year  = if month > 2 { c - 4716 } else { c - 4715 };

    let hours  = (f * 24.0).floor() as u32;
    let mins   = ((f * 24.0 - hours as f64) * 60.0).floor() as u32;
    let secs   = (((f * 24.0 - hours as f64) * 60.0 - mins as f64) * 60.0).round() as u32;
    let secs   = secs.min(59); // avoid 60 from rounding

    if year <= 0 {
        let bce = (1 - year) as u32;
        format!("{bce:04}-{month:02}-{day:02} {hours:02}:{mins:02}:{secs:02} BCE")
    } else {
        format!("{year:04}-{month:02}-{day:02} {hours:02}:{mins:02}:{secs:02}")
    }
}
