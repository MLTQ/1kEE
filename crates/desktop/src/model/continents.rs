//! Exclusive broadcast regions, using broad geographic seams rather than borders.
use super::GeoPoint;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Continent {
    NorthAmerica,
    SouthAmerica,
    Europe,
    Africa,
    Asia,
    Oceania,
    Antarctica,
}

impl Continent {
    pub const ALL: [Self; 7] = [
        Self::NorthAmerica,
        Self::SouthAmerica,
        Self::Europe,
        Self::Africa,
        Self::Asia,
        Self::Oceania,
        Self::Antarctica,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::NorthAmerica => "North America",
            Self::SouthAmerica => "South America",
            Self::Europe => "Europe",
            Self::Africa => "Africa",
            Self::Asia => "Asia",
            Self::Oceania => "Oceania",
            Self::Antarctica => "Antarctica",
        }
    }

    pub fn center(self) -> GeoPoint {
        let (lat, lon) = match self {
            Self::NorthAmerica => (40.0, -100.0),
            Self::SouthAmerica => (-20.0, -60.0),
            Self::Europe => (53.0, 20.0),
            Self::Africa => (2.0, 20.0),
            Self::Asia => (35.0, 95.0),
            Self::Oceania => (-22.0, 155.0),
            Self::Antarctica => (-85.0, 0.0),
        };
        GeoPoint { lat, lon }
    }

    /// Every valid position belongs to exactly one region, including oceans.
    /// Seams approximate Panama, the Mediterranean/Red Sea, the Urals/Caucasus,
    /// and the western Pacific. They are presentation regions, not country claims.
    pub fn at(point: GeoPoint) -> Option<Self> {
        let GeoPoint { lat, lon } = point;
        if !lat.is_finite()
            || !lon.is_finite()
            || !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
        {
            return None;
        }
        let lon = (lon + 180.0).rem_euclid(360.0) - 180.0;
        Some(if lat < -60.0 {
            Self::Antarctica
        } else if (lon >= 110.0 && lat < -10.0)
            || (lon >= 130.0 && lat < 0.0)
            || (lon >= 145.0 && lat < 20.0)
            || (lon < -110.0 && lat < 0.0)
            || (lon < -170.0 && lat < 30.0)
            || (lon < -140.0 && lat < 15.0)
        {
            Self::Oceania
        } else if (-170.0..-30.0).contains(&lon)
            || (lon.abs() >= 170.0 && (50.0..56.0).contains(&lat))
        {
            // Caribbean and Central America stay north of a Panama/Colombia seam.
            let south = inside(
                lon,
                lat,
                &[
                    (-110.0, -60.0),
                    (-30.0, -60.0),
                    (-30.0, 15.0),
                    (-60.0, 15.0),
                    (-73.0, 12.0),
                    (-77.0, 8.5),
                    (-82.0, 7.0),
                    (-110.0, 0.0),
                ],
            );
            if south {
                Self::SouthAmerica
            } else {
                Self::NorthAmerica
            }
        } else if inside(
            lon,
            lat,
            &[
                (-30.0, -60.0),
                (60.0, -60.0),
                (60.0, 0.0),
                (44.0, 12.0),
                (34.0, 28.0),
                (33.0, 31.5),
                (25.0, 33.0),
                (12.0, 33.0),
                (11.0, 37.4),
                (8.0, 37.4),
                (-6.0, 36.0),
                (-30.0, 36.0),
            ],
        ) {
            Self::Africa
        } else if inside(
            lon,
            lat,
            &[
                (-30.0, 36.0),
                (-6.0, 36.0),
                (10.0, 35.0),
                (23.0, 35.0),
                (29.0, 41.0),
                (42.0, 42.0),
                (50.0, 47.0),
                (60.0, 50.0),
                (60.0, 90.0),
                (-30.0, 90.0),
            ],
        ) {
            Self::Europe
        } else {
            Self::Asia
        })
    }
}

fn inside(x: f32, y: f32, polygon: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut previous = polygon[polygon.len() - 1];
    for &(px, py) in polygon {
        let (qx, qy) = previous;
        if (py > y) != (qy > y) && x < (qx - px) * (y - py) / (qy - py) + px {
            inside = !inside;
        }
        previous = (px, py);
    }
    inside
}
