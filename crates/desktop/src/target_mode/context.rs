//! Existing map context draped on the independent DEM, clipped to the circle.
use super::{Batch, Terrain, Vertex, serial};
use crate::model::{AppModel, GeoJsonGeometry, GeoPoint};
use lidar_pipeline::region::Region;
use std::{path::PathBuf, sync::Arc};

#[derive(Default)]
pub(crate) struct ContextInput {
    root: Option<PathBuf>,
    lines: Vec<(Vec<GeoPoint>, [u8; 4])>,
    labels: Vec<(GeoPoint, String)>,
    limited: bool,
}
pub(crate) struct ContextGeometry {
    pub batch: Arc<Batch>,
    pub labels: Vec<([f32; 3], String)>,
    pub label: String,
}
impl ContextInput {
    pub fn with_root(mut self, root: Option<PathBuf>) -> Self {
        self.root = root;
        self
    }
    pub fn capture(model: &AppModel, region: Region) -> Self {
        let mut input = Self::default();
        let bounds = region.bounds();
        let mut count = 0;
        let mut line_points = 0;
        let layers = model.geojson_layers.iter().chain(
            model
                .submarine_cable_layers
                .iter()
                .filter(|_| model.show_submarine_cables),
        );
        for layer in layers.filter(|l| l.visible) {
            for feature in &layer.features {
                if count >= 100_000 {
                    input.limited = true;
                    break;
                }
                count += 1;
                let color = feature.color.unwrap_or(layer.color);
                let mut line = |points: &Vec<GeoPoint>| {
                    if intersects(points, bounds) {
                        if line_points + points.len() <= 100_000 {
                            line_points += points.len();
                            input.lines.push((points.clone(), color));
                        } else {
                            input.limited = true;
                        }
                    }
                };
                match &feature.geometry {
                    GeoJsonGeometry::Point(p) => {
                        if region.contains(p.lon.into(), p.lat.into()) {
                            input.labels.push((
                                *p,
                                feature.label.clone().unwrap_or_else(|| layer.name.clone()),
                            ));
                        }
                    }
                    GeoJsonGeometry::LineString(p) => line(p),
                    GeoJsonGeometry::MultiLineString(lines) | GeoJsonGeometry::Polygon(lines) => {
                        for p in lines {
                            line(p);
                        }
                    }
                    GeoJsonGeometry::MultiPolygon(polygons) => {
                        for rings in polygons {
                            for p in rings {
                                line(p);
                            }
                        }
                    }
                }
            }
        }
        for event in &model.events {
            if region.contains(event.location.lon.into(), event.location.lat.into()) {
                input.labels.push((event.location, "Event".into()));
            }
        }
        for camera in &model.cameras {
            if region.contains(camera.location.lon.into(), camera.location.lat.into()) {
                input.labels.push((camera.location, "Camera".into()));
            }
        }
        input
    }
}
fn intersects(points: &[GeoPoint], b: [f64; 4]) -> bool {
    let mut p = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for q in points {
        p = [
            p[0].min(q.lon),
            p[1].min(q.lat),
            p[2].max(q.lon),
            p[3].max(q.lat),
        ];
    }
    f64::from(p[0]) <= b[2]
        && f64::from(p[2]) >= b[0]
        && f64::from(p[1]) <= b[3]
        && f64::from(p[3]) >= b[1]
}
pub(super) fn build(
    region: Region,
    mut input: ContextInput,
    terrain: Option<Arc<Terrain>>,
    active: &std::path::Path,
) -> Result<ContextGeometry, String> {
    let Some(terrain) = terrain else {
        return Err("Map context needs a DEM for ground placement".into());
    };
    // Full-resolution cached vector cells and roads are bounded to a 5 km
    // central detail window for regional selections. Imported context remains
    // clipped to the full Target; no regional planet-cell fanout is launched.
    let detail = Region {
        radius_m: region.radius_m.min(5000.),
        ..region
    };
    let [w, s, e, n] = detail.bounds();
    if w >= -180.
        && e <= 180.
        && let Some(root) = &input.root
    {
        let bounds = crate::osm_ingest::GeoBounds {
            min_lon: w as f32,
            min_lat: s as f32,
            max_lon: e as f32,
            max_lat: n as f32,
        };
        for road in crate::osm_ingest::load_roads_for_bounds(
            Some(root),
            bounds,
            14,
            crate::osm_ingest::RoadLayerKind::All,
        ) {
            if intersects(&road.points, [w, s, e, n]) {
                input.lines.push((road.points, [227, 151, 82, 255]));
            }
        }
        for water in crate::osm_ingest::water::load_water_for_bounds(Some(root), bounds, 14) {
            if intersects(&water.points, [w, s, e, n]) {
                input.lines.push((water.points, [67, 153, 217, 255]));
            }
        }
        for prefix in [
            "bldg", "rail", "powr", "pipe", "aero", "port", "govt", "inds", "comm", "milt", "surv",
            "tree",
        ] {
            if !active.is_file() {
                return Err("Target closed".into());
            }
            for feature in crate::panels::world_map::cell_loader::load_features_from_cells(
                root, prefix, bounds,
            ) {
                if intersects(&feature.points, [w, s, e, n]) {
                    input.lines.push((feature.points, [133, 157, 171, 255]));
                }
            }
        }
    }
    let mut points = Vec::new();
    'lines: for (line, color) in input.lines {
        if !active.is_file() {
            return Err("Target closed".into());
        }
        for pair in line.windows(2) {
            if points.len() > 500_000 {
                input.limited = true;
                break 'lines;
            }
            let a = region.offset(pair[0].lon.into(), pair[0].lat.into());
            let b = region.offset(pair[1].lon.into(), pair[1].lat.into());
            let Some((a, b)) = clip(a, b, region.radius_m) else {
                continue;
            };
            let steps = ((b[0] - a[0]).hypot(b[1] - a[1]) / 10.).ceil().max(1.) as usize;
            let mut previous = None;
            for i in 0..=steps {
                let t = i as f64 / steps as f64;
                let xy = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                let ll = region.location(xy[0], xy[1]);
                let current = terrain.sample(ll[0], ll[1]).map(|z| Vertex {
                    position: [xy[0] as f32, xy[1] as f32, z],
                    color,
                    class: 0,
                });
                if let (Some(a), Some(b)) = (previous, current) {
                    points.extend([a, b]);
                }
                previous = current;
            }
        }
    }
    let labels = input
        .labels
        .into_iter()
        .take(256)
        .filter_map(|(p, label)| {
            let z = terrain.sample(p.lon.into(), p.lat.into())?;
            let [x, y] = region.offset(p.lon.into(), p.lat.into());
            Some(([x as f32, y as f32, z], label))
        })
        .collect::<Vec<_>>();
    let mut label = format!(
        "{} context segments · {} markers · DEM-draped{}",
        points.len() / 2,
        labels.len(),
        if input.limited {
            " · display limit reached"
        } else {
            ""
        }
    );
    if region.radius_m > 5000. {
        label.push_str(" · cached detail within central 5 km radius");
    }
    Ok(ContextGeometry {
        batch: Arc::new(Batch {
            source: None,
            id: serial(),
            points,
        }),
        labels,
        label,
    })
}
fn clip(a: [f64; 2], b: [f64; 2], radius: f64) -> Option<([f64; 2], [f64; 2])> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let aa = d[0] * d[0] + d[1] * d[1];
    if aa < 1e-12 {
        return None;
    }
    let bb = 2. * (a[0] * d[0] + a[1] * d[1]);
    let cc = a[0] * a[0] + a[1] * a[1] - radius * radius;
    let disc = bb * bb - 4. * aa * cc;
    if disc < 0. {
        return None;
    }
    let lo = ((-bb - disc.sqrt()) / (2. * aa)).max(0.);
    let hi = ((-bb + disc.sqrt()) / (2. * aa)).min(1.);
    (lo <= hi).then(|| {
        (
            [a[0] + d[0] * lo, a[1] + d[1] * lo],
            [a[0] + d[0] * hi, a[1] + d[1] * hi],
        )
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_clips_crossing_segments_even_when_both_endpoints_are_outside() {
        assert_eq!(
            clip([-2000., 0.], [2000., 0.], 1000.),
            Some(([-1000., 0.], [1000., 0.]))
        );
        assert!(clip([-2000., 1100.], [2000., 1100.], 1000.).is_none());
        assert_eq!(
            clip([0., 0.], [2000., 0.], 1000.),
            Some(([0., 0.], [1000., 0.]))
        );
    }
}
