//! Bounded, source-derived contour curtains. Never interpolate terrain heights.
use super::{ContourPath, LocalSegmentInstance};
use std::sync::Arc;

pub(super) const TILE_SEGMENTS: usize = 4096;

#[derive(Clone, Default)]
pub(crate) struct Geometry {
    pub segments: Arc<Vec<LocalSegmentInstance>>,
    /// Maximum geographic displacement introduced by radial simplification.
    pub error_deg: f32,
    pub bounds: [f32; 4],
}

/// Runs alongside line instance construction on its existing worker. Sparse
/// elevation levels only weaken occlusion; they cannot invent a taller ridge.
pub(super) fn build(contours: &[ContourPath]) -> Geometry {
    let mut levels: Vec<_> = contours
        .iter()
        .filter_map(|c| (c.elevation_m.is_finite() && c.points.len() >= 2).then_some(c.elevation_m))
        .collect();
    levels.sort_by(f32::total_cmp);
    levels.dedup();
    if levels.is_empty() {
        return Geometry::default();
    }
    let stride = levels.len().div_ceil(24);
    let selected: Vec<_> = levels.iter().rev().step_by(stride).copied().collect();
    let mut paths: Vec<_> = contours
        .iter()
        .filter(|c| selected.contains(&c.elevation_m) && c.points.len() >= 2)
        .collect();
    // Prefer long connected ridges if there are more paths than the budget.
    paths.sort_by_key(|c| std::cmp::Reverse(c.points.len()));
    paths.truncate(TILE_SEGMENTS / 4);
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for c in &paths {
        for p in &c.points {
            if p.lon.is_finite() && p.lat.is_finite() {
                bounds[0] = bounds[0].min(p.lon);
                bounds[1] = bounds[1].min(p.lat);
                bounds[2] = bounds[2].max(p.lon);
                bounds[3] = bounds[3].max(p.lat);
            }
        }
    }
    let mut error = ((bounds[2] - bounds[0]).max(bounds[3] - bounds[1]) / 1024.0).max(1e-7);
    for _ in 0..24 {
        let mut result = Vec::new();
        for c in &paths {
            let mut anchor = None;
            let mut last = None;
            for p in &c.points {
                if !p.lon.is_finite() || !p.lat.is_finite() {
                    // Never connect across invalid source points.
                    if let (Some(a), Some(b)) = (anchor, last) {
                        emit(&mut result, a, b);
                    }
                    anchor = None;
                    last = None;
                    continue;
                }
                let b = [p.lon, p.lat, c.elevation_m];
                if let Some(a) = anchor {
                    let dx: f32 = b[0] - a[0];
                    let dy: f32 = b[1] - a[1];
                    if dx * dx + dy * dy >= error * error {
                        emit(&mut result, a, b);
                        anchor = Some(b);
                    }
                } else {
                    anchor = Some(b);
                }
                last = Some(b);
            }
            if let (Some(a), Some(b)) = (anchor, last) {
                emit(&mut result, a, b);
            }
            if result.len() > TILE_SEGMENTS {
                break;
            }
        }
        if result.len() <= TILE_SEGMENTS {
            return Geometry {
                segments: Arc::new(result),
                error_deg: error,
                bounds,
            };
        }
        error *= 2.0;
    }
    // Invalid/pathological input must never make uploads or paint unbounded.
    Geometry::default()
}
fn emit(out: &mut Vec<LocalSegmentInstance>, a: [f32; 3], b: [f32; 3]) {
    if a != b {
        out.push(LocalSegmentInstance::line(
            a,
            b,
            egui::Color32::WHITE,
            false,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::GeoPoint;
    #[test]
    fn preserves_heights_endpoints_and_disconnected_paths_with_bounded_output() {
        let paths: Vec<_> = (0..100)
            .map(|h| ContourPath {
                elevation_m: h as f32 - 50.0,
                points: (0..2000)
                    .map(|i| GeoPoint {
                        lon: i as f32 * 0.00001,
                        lat: h as f32 * 0.0001,
                    })
                    .collect(),
            })
            .collect();
        let geometry = build(&paths);
        assert!(!geometry.segments.is_empty());
        assert!(geometry.segments.len() <= TILE_SEGMENTS);
        assert!(geometry.segments.iter().any(|s| s.a[2] == 49.0));
        for s in geometry.segments.iter() {
            assert_eq!(s.a[2], s.b[2]);
            assert_eq!(s.a[1], s.b[1]);
            assert!(s.a[0] >= 0.0 && s.b[0] <= 0.019991);
        }
    }
    #[test]
    fn invalid_points_do_not_bridge_gaps() {
        let geometry = build(&[ContourPath {
            elevation_m: 100.0,
            points: vec![
                GeoPoint { lat: 0.0, lon: 0.0 },
                GeoPoint { lat: 0.0, lon: 0.1 },
                GeoPoint {
                    lat: f32::NAN,
                    lon: 0.2,
                },
                GeoPoint { lat: 0.0, lon: 0.3 },
                GeoPoint { lat: 0.0, lon: 0.4 },
            ],
        }]);
        assert_eq!(geometry.segments.len(), 2);
        assert!(geometry.segments.iter().all(|s| s.b[0] - s.a[0] < 0.11));
    }
}
