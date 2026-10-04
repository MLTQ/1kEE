//! Preserve complete Earth cores while bounding legacy overlapping geometry.
use super::{CacheKey, ContourPath, GeoPoint, select_contour_geometry, simplify_line};
use crate::model::ActiveBody;
use crate::panels::world_map::{local_contour_pass::LocalTileId, local_terrain_scene::composition};
use tile_archive::contour_grid::{Bounds, clip_line};

#[derive(Clone, Copy)]
pub(super) enum ReadSelection {
    /// Globe and Moon/Mars retain their existing footprint and budget policy.
    WholeTile(usize),
    /// Every Earth contour in the owned core, including short edge fragments.
    EarthCore,
}

pub(super) struct TileSelection {
    core: Option<Bounds>,
    simplify_step: usize,
    feature_budget: usize,
}

impl ReadSelection {
    pub(super) fn tile(self, key: &CacheKey, simplify_step: usize) -> TileSelection {
        let core = match self {
            Self::WholeTile(_) => None,
            Self::EarthCore => Some(composition::core(
                ActiveBody::Earth,
                LocalTileId {
                    zoom_bucket: key.zoom_bucket,
                    lat_bucket: key.lat_bucket,
                    lon_bucket: key.lon_bucket,
                },
            )),
        };
        TileSelection {
            core,
            simplify_step,
            feature_budget: match self {
                Self::WholeTile(budget) => budget,
                Self::EarthCore => usize::MAX,
            },
        }
    }
}

impl TileSelection {
    pub(super) fn append(
        &self,
        contours: &mut Vec<ContourPath>,
        lines: impl IntoIterator<Item = Vec<GeoPoint>>,
        elevation_m: f32,
    ) {
        for line in lines {
            if let Some(core) = self.core {
                // Clip before simplifying: an edge intersection must not depend
                // on the vertex-stride phase of either neighboring source.
                let points: Vec<_> = line
                    .iter()
                    .map(|p| (f64::from(p.lon), f64::from(p.lat)))
                    .collect();
                for part in clip_line(&points, core) {
                    let mut points: Vec<_> = part
                        .into_iter()
                        .map(|(lon, lat)| GeoPoint {
                            lon: lon as f32,
                            lat: lat as f32,
                        })
                        .collect();
                    points.dedup_by(|a, b| a == b);
                    self.push(contours, points, elevation_m);
                }
            } else {
                self.push(contours, line, elevation_m);
            }
        }
    }

    fn push(&self, contours: &mut Vec<ContourPath>, points: Vec<GeoPoint>, elevation_m: f32) {
        if points.len() >= 2 {
            contours.push(ContourPath {
                elevation_m,
                points: simplify_line(points, self.simplify_step),
            });
        }
    }

    pub(super) fn finish(&self, contours: &mut Vec<ContourPath>) {
        select_contour_geometry(contours, self.feature_budget);
    }
}
