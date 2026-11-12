//! Offline Natural Earth lookup for the survey browser. Each polygon part stays on
//! its original side of the antimeridian, including the polar and ocean polygons.
use std::sync::LazyLock;

use serde::Deserialize;

type Ring = Vec<[f32; 2]>;
pub(super) type Place = (&'static str, &'static str);

#[derive(Deserialize)]
struct Collection {
    features: Vec<Feature>,
}
#[derive(Deserialize)]
struct Feature {
    properties: serde_json::Value,
    geometry: Geometry,
}
#[derive(Deserialize)]
#[serde(tag = "type", content = "coordinates")]
enum Geometry {
    Polygon(Vec<Ring>),
    MultiPolygon(Vec<Vec<Ring>>),
}

struct Area {
    group: String,
    name: String,
    bounds: [f32; 4],
    rings: Vec<Ring>,
}

fn areas(json: &str, ocean: bool) -> Vec<Area> {
    let collection: Collection = serde_json::from_str(json).expect("bundled Natural Earth regions");
    let mut areas = Vec::new();
    for f in collection.features {
        let props = f.properties;
        let (group, name) = if ocean {
            ("Oceans", props["name"].as_str().unwrap())
        } else {
            (
                props["CONTINENT"].as_str().unwrap(),
                props["NAME_LONG"].as_str().unwrap(),
            )
        };
        let parts = match f.geometry {
            Geometry::Polygon(p) => vec![p],
            Geometry::MultiPolygon(parts) => parts,
        };
        for rings in parts {
            let mut bounds = [180_f32, 90_f32, -180_f32, -90_f32];
            for &[x, y] in &rings[0] {
                bounds = [
                    bounds[0].min(x),
                    bounds[1].min(y),
                    bounds[2].max(x),
                    bounds[3].max(y),
                ];
            }
            areas.push(Area {
                group: group.into(),
                name: name.into(),
                bounds,
                rings,
            });
        }
    }
    areas
}

static LAND: LazyLock<Vec<Area>> = LazyLock::new(|| {
    areas(
        include_str!("../../../cache_builder/assets/ne_110m_countries.json"),
        false,
    )
});
// More specific named seas and gulfs precede the ocean basins.
static SEA: LazyLock<Vec<Area>> =
    LazyLock::new(|| areas(include_str!("marine_regions.geojson"), true));

impl Area {
    fn contains(&self, x: f32, y: f32) -> bool {
        let [w, s, e, n] = self.bounds;
        if x < w || x > e || y < s || y > n {
            return false;
        }
        let mut inside = false;
        for edge in self.rings.iter().flat_map(|r| r.windows(2)) {
            let ([ax, ay], [bx, by]) = (edge[0], edge[1]);
            if (ay > y) != (by > y) && x < ax + (bx - ax) * (y - ay) / (by - ay) {
                inside = !inside;
            }
        }
        inside
    }

    fn coast_distance(&self, x: f32, y: f32, scale: f32) -> Option<f32> {
        // About 30 km accommodates generalized shorelines without claiming the open sea.
        let [w, s, e, n] = self.bounds;
        let limit = 0.27;
        if x < w - limit / scale || x > e + limit / scale || y < s - limit || y > n + limit {
            return None;
        }
        self.rings[0]
            .windows(2)
            .map(|edge| {
                let ([ax, ay], [bx, by]) = (edge[0], edge[1]);
                let (dx, dy) = ((bx - ax) * scale, by - ay);
                let (px, py) = ((x - ax) * scale, y - ay);
                let u = ((px * dx + py * dy) / (dx * dx + dy * dy).max(f32::EPSILON)).clamp(0., 1.);
                (px - u * dx).powi(2) + (py - u * dy).powi(2)
            })
            .filter(|d| *d <= limit * limit)
            .min_by(f32::total_cmp)
    }

    fn place(&'static self) -> Place {
        (&self.group, &self.name)
    }
}

// A coastal collection can be browsed from its country as well as its offshore region.
pub(super) fn places(source: &str, lon: f32, lat: f32) -> [Option<Place>; 2] {
    let national = match source {
        "eba" | "nasa-brazil" => return [Some(("South America", "Brazil")), None],
        "swisssurface3d" => return [Some(("Europe", "Switzerland")), None],
        "linz-coastal" => Some(("Oceania", "New Zealand")),
        "infomar-lidar" => Some(("Europe", "Ireland")),
        _ => None,
    };
    let land = LAND.iter().find(|a| a.contains(lon, lat)).map(Area::place);
    let water = if land.is_none() || national.is_some() {
        SEA.iter().find(|a| a.contains(lon, lat)).map(Area::place)
    } else {
        None
    };
    let country = national.or(land).or_else(|| {
        let scale = lat.to_radians().cos().max(0.05);
        LAND.iter()
            .filter_map(|a| a.coast_distance(lon, lat, scale).map(|d| (d, a)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, a)| a.place())
    });
    if country.is_none() && water.is_none() {
        [Some(("Other locations", "Unassigned")), None]
    } else {
        [country, water]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cities_seas_and_open_ocean_resolve_offline() {
        for (x, y, place) in [
            (-60., -3., ("South America", "Brazil")),
            (12.5, 41.9, ("Europe", "Italy")),
            (-90., 25., ("Oceans", "Gulf of Mexico")),
            (170., 25., ("Oceans", "North Pacific Ocean")),
            (-30., -25., ("Oceans", "South Atlantic Ocean")),
        ] {
            assert!(
                places("opentopography", x, y).contains(&Some(place)),
                "{x},{y}: {:?}",
                places("opentopography", x, y)
            );
        }
    }
}
