//! Exclusive broadcast regions, using broad geographic seams rather than borders.
use super::GeoPoint;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecapRegion {
    NorthAmerica,
    SouthAmerica,
    Europe,
    Africa,
    MiddleEast,
    Asia,
    Oceania,
    Antarctica,
}

impl RecapRegion {
    pub const ALL: [Self; 8] = [
        Self::NorthAmerica,
        Self::SouthAmerica,
        Self::Europe,
        Self::Africa,
        Self::MiddleEast,
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
            Self::MiddleEast => "Middle East",
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
            Self::MiddleEast => (25.0, 45.0),
            Self::Asia => (35.0, 95.0),
            Self::Oceania => (-22.0, 155.0),
            Self::Antarctica => (-85.0, 0.0),
        };
        GeoPoint { lat, lon }
    }

    /// Individually tuned closer views, rather than the old shared 1.15 overview.
    pub fn zoom(self) -> f32 {
        match self {
            Self::NorthAmerica => 1.6,
            Self::SouthAmerica => 1.7,
            Self::Europe => 2.0,
            Self::Africa => 1.6,
            Self::MiddleEast => 2.2,
            Self::Asia => 1.5,
            Self::Oceania => 1.6,
            Self::Antarctica => 1.8,
        }
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
        // Assign this dedicated broadcast zone before its neighboring continents.
        let middle_east = inside(
            lon,
            lat,
            &[
                (26.0, 42.0),
                (44.0, 42.0),
                (44.5, 39.5),
                (48.5, 39.8),
                (54.5, 38.5),
                (60.5, 37.5),
                (61.0, 36.0),
                (60.5, 34.0),
                (61.0, 29.0),
                (63.5, 27.0),
                (61.5, 25.0),
                (60.0, 22.0),
                (58.0, 18.0),
                (54.0, 12.0),
                (42.5, 12.0),
                (42.5, 16.0),
                (36.5, 22.0),
                (25.0, 22.0),
                (25.0, 31.5),
                (26.0, 34.0),
            ],
        );
        Some(if middle_east {
            Self::MiddleEast
        } else if lat < -60.0 {
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
