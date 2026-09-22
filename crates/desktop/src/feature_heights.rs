use crate::model::GeoPoint;

pub fn prepare(
    points: &[GeoPoint],
    elevations: Option<&[f32]>,
    max_points: usize,
    offset: f32,
    mut sample: impl FnMut(GeoPoint) -> Option<f32>,
) -> Vec<(GeoPoint, f32)> {
    if points.is_empty() {
        return Vec::new();
    }
    let max_points = max_points.max(2);
    let stride = if points.len() <= max_points {
        1
    } else {
        ((points.len() - 1) as f32 / (max_points - 1) as f32).ceil() as usize
    };
    let mut indices = (0..points.len().saturating_sub(1))
        .step_by(stride)
        .collect::<Vec<_>>();
    let last = points.len() - 1;
    if stride == 1 || indices.last().is_none_or(|&i| points[i] != points[last]) {
        indices.push(last);
    }
    let elevations = elevations.filter(|e| e.len() == points.len());
    indices
        .into_iter()
        .map(|i| {
            let baked = elevations
                .and_then(|e| e.get(i))
                .copied()
                .filter(|e| e.is_finite());
            let height = baked.or_else(|| sample(points[i])).unwrap_or(0.0);
            (points[i], height + offset)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn baked_heights_survive_thinning_without_source_reads() {
        let points = (0..10)
            .map(|i| GeoPoint {
                lat: i as f32,
                lon: 0.0,
            })
            .collect::<Vec<_>>();
        let heights = (0..10).map(|i| 100.0 + i as f32).collect::<Vec<_>>();
        let output = prepare(&points, Some(&heights), 4, 3.0, |_| {
            panic!("baked data must not sample terrain")
        });
        assert_eq!(
            output.iter().map(|(p, h)| (p.lat, *h)).collect::<Vec<_>>(),
            vec![(0.0, 103.0), (3.0, 106.0), (6.0, 109.0), (9.0, 112.0)]
        );
        let mut calls = 0;
        prepare(&points, Some(&[0.0]), 4, 3.0, |_| {
            calls += 1;
            Some(4.0)
        });
        assert_eq!(calls, 4);
    }
}
