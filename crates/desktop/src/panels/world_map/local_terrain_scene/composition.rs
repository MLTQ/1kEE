//! Compose disjoint geographic pieces of ready LODs on a background worker.
use super::lod;
use crate::model::ActiveBody;
use crate::panels::world_map::{
    contour_asset::{ContourPath, LocalTileGeometry, residency::Bounds},
    local_contour_pass::LocalTileId,
};
use std::{collections::HashMap, sync::Arc};
use tile_archive::contour_grid::{Bounds as Rect, clip_line};

#[derive(Clone)]
pub(crate) struct Source {
    pub tile: LocalTileGeometry,
    pub priority: u64,
}

#[derive(Clone)]
pub(crate) struct Piece {
    pub source: Arc<Vec<ContourPath>>,
    pub regions: Vec<Rect>,
    pub tile: LocalTileGeometry,
}

#[derive(Clone, Default)]
pub(crate) struct Frame {
    pub id: u64,
    pub tiles: Vec<LocalTileGeometry>,
    pub contours: Option<Arc<Vec<ContourPath>>>,
}

pub(crate) struct Result {
    pub frame: Frame,
    pub pieces: HashMap<LocalTileId, Piece>,
}

pub(crate) fn core(body: ActiveBody, id: LocalTileId) -> Rect {
    let step = f64::from(lod::step(
        body,
        lod::ZOOMS[id.zoom_bucket.clamp(0, 10) as usize],
    ));
    Rect {
        min_lon: (f64::from(id.lon_bucket) - 0.5) * step,
        max_lon: (f64::from(id.lon_bucket) + 0.5) * step,
        min_lat: (f64::from(id.lat_bucket) - 0.5) * step,
        max_lat: (f64::from(id.lat_bucket) + 0.5) * step,
    }
}

/// Only a tile's owned core can draw, including for legacy overlapping tiles.
pub(crate) fn visible_bounds(body: ActiveBody, tile: &LocalTileGeometry) -> Bounds {
    let r = core(body, tile.id);
    Bounds {
        min: [
            r.min_lon as f32,
            r.min_lat as f32,
            tile.bounds.map_or(0.0, |b| b.min[2]),
        ],
        max: [
            r.max_lon as f32,
            r.max_lat as f32,
            tile.bounds.map_or(0.0, |b| b.max[2]),
        ],
    }
}

pub(crate) fn compose(
    body: ActiveBody,
    mut sources: Vec<Source>,
    previous: HashMap<LocalTileId, Piece>,
) -> Result {
    // Deterministic order preserves fragment/Arc identity when readers arrive
    // in a different order. Arrival within one tier never changes ownership.
    sources.sort_by_key(|s| {
        (
            std::cmp::Reverse(s.priority),
            s.tile.id.zoom_bucket,
            s.tile.id.lat_bucket,
            s.tile.id.lon_bucket,
        )
    });
    let mut covered = Vec::<(u64, Rect)>::new();
    let mut pieces = HashMap::new();
    let mut tiles = Vec::new();
    for source in sources {
        let id = source.tile.id;
        let owned = core(body, id);
        let mut regions = vec![owned];
        for &(priority, replacement) in &covered {
            if priority > source.priority {
                regions = regions
                    .into_iter()
                    .flat_map(|r| subtract(r, replacement))
                    .collect();
                if regions.is_empty() {
                    break;
                }
            }
        }
        covered.push((source.priority, owned));
        if regions.is_empty() {
            continue;
        }
        let tile = if let Some(cached) = previous.get(&id)
            && Arc::ptr_eq(&cached.source, &source.tile.contours)
            && cached.regions == regions
        {
            cached.tile.clone()
        } else {
            let contours = clip_contours(&source.tile, &regions);
            let bounds = Bounds::from_contours(&contours);
            LocalTileGeometry {
                id,
                contours,
                bounds,
            }
        };
        pieces.insert(
            id,
            Piece {
                source: source.tile.contours,
                regions,
                tile: tile.clone(),
            },
        );
        tiles.push(tile);
    }
    let contours = (!tiles.is_empty()).then(|| {
        Arc::new(
            tiles
                .iter()
                .flat_map(|t| t.contours.iter().cloned())
                .collect(),
        )
    });
    Result {
        frame: Frame {
            id: 0,
            tiles,
            contours,
        },
        pieces,
    }
}

/// Four nonoverlapping strips. Adjacent tiles share exact f64 boundaries.
fn subtract(a: Rect, b: Rect) -> Vec<Rect> {
    let left = a.min_lon.max(b.min_lon);
    let right = a.max_lon.min(b.max_lon);
    let bottom = a.min_lat.max(b.min_lat);
    let top = a.max_lat.min(b.max_lat);
    if left >= right || bottom >= top {
        return vec![a];
    }
    [
        Rect { max_lon: left, ..a },
        Rect {
            min_lon: right,
            ..a
        },
        Rect {
            min_lon: left,
            max_lon: right,
            max_lat: bottom,
            ..a
        },
        Rect {
            min_lon: left,
            max_lon: right,
            min_lat: top,
            ..a
        },
    ]
    .into_iter()
    .filter(|r| r.min_lon < r.max_lon && r.min_lat < r.max_lat)
    .collect()
}

fn clip_contours(tile: &LocalTileGeometry, regions: &[Rect]) -> Arc<Vec<ContourPath>> {
    // Modern core tiles in unobstructed regions need no geometry copy or GPU
    // rebuild. Clipping is normally limited to legacy tiles and LOD borders.
    if tile.contours.is_empty()
        || tile.bounds.is_some_and(|b| {
            regions.iter().any(|r| {
                b.min[0] >= r.min_lon as f32
                    && b.max[0] <= r.max_lon as f32
                    && b.min[1] >= r.min_lat as f32
                    && b.max[1] <= r.max_lat as f32
            })
        })
    {
        return tile.contours.clone();
    }
    let mut clipped = Vec::new();
    for contour in tile.contours.iter() {
        let points: Vec<_> = contour
            .points
            .iter()
            .map(|p| (f64::from(p.lon), f64::from(p.lat)))
            .collect();
        let bounds = points.iter().fold(
            [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ],
            |mut b, &(x, y)| {
                b[0] = b[0].min(x);
                b[1] = b[1].min(y);
                b[2] = b[2].max(x);
                b[3] = b[3].max(y);
                b
            },
        );
        for &region in regions {
            if bounds[2] < region.min_lon
                || bounds[0] > region.max_lon
                || bounds[3] < region.min_lat
                || bounds[1] > region.max_lat
            {
                continue;
            }
            for part in clip_line(&points, region) {
                let mut points: Vec<_> = part
                    .into_iter()
                    .map(|(lon, lat)| crate::model::GeoPoint {
                        lon: lon as f32,
                        lat: lat as f32,
                    })
                    .collect();
                points.dedup_by(|a, b| a.lon == b.lon && a.lat == b.lat);
                if points.len() >= 2 {
                    clipped.push(ContourPath {
                        elevation_m: contour.elevation_m,
                        points,
                    });
                }
            }
        }
    }
    Arc::new(clipped)
}

#[cfg(test)]
#[path = "composition_tests.rs"]
mod tests;
