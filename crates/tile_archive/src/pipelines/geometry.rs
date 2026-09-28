use cell_format::CellPoint;

/// Split every segment at cell edges, preserving continuity and dateline sides.
pub(super) fn split(points: &[[f64; 2]]) -> Vec<((i32, i32), Vec<CellPoint>)> {
    let mut out: Vec<((i32, i32), Vec<CellPoint>)> = Vec::new();
    for pair in points.windows(2) {
        let a = pair[0];
        let mut b = pair[1];
        while b[0] - a[0] > 180.0 {
            b[0] -= 360.0;
        }
        while b[0] - a[0] < -180.0 {
            b[0] += 360.0;
        }
        if a == b {
            continue;
        }
        let mut cuts = vec![0.0, 1.0];
        for axis in 0..2 {
            let delta = b[axis] - a[axis];
            if delta == 0.0 {
                continue;
            }
            let lo = (a[axis].min(b[axis]) * 4.0).floor() as i32 + 1;
            let hi = (a[axis].max(b[axis]) * 4.0).ceil() as i32;
            for edge in lo..hi {
                let t = (edge as f64 / 4.0 - a[axis]) / delta;
                if t > 0.0 && t < 1.0 {
                    cuts.push(t);
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
        for ts in cuts.windows(2) {
            let mid = (ts[0] + ts[1]) * 0.5;
            let mid_lon = a[0] + (b[0] - a[0]) * mid;
            let wrap = ((mid_lon + 180.0) / 360.0).floor() * 360.0;
            let cell = (
                ((a[1] + (b[1] - a[1]) * mid) * 4.0).floor() as i32,
                ((mid_lon - wrap) * 4.0).floor() as i32,
            );
            let cell = (cell.0.clamp(-360, 359), cell.1.clamp(-720, 719));
            let point = |t| CellPoint {
                lon: (a[0] + (b[0] - a[0]) * t - wrap) as f32,
                lat: (a[1] + (b[1] - a[1]) * t) as f32,
            };
            let (p, q) = (point(ts[0]), point(ts[1]));
            if let Some((last_cell, line)) = out.last_mut()
                && *last_cell == cell
                && line.last() == Some(&p)
            {
                if line.last() != Some(&q) {
                    line.push(q);
                }
            } else if p != q {
                out.push((cell, vec![p, q]));
            }
        }
    }
    out
}

/// Iterative Douglas–Peucker; no recursion or fixed point budget.
pub(super) fn simplify(points: &[CellPoint], tolerance: f64) -> Vec<CellPoint> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let p = points[a];
        let q = points[b];
        let dx = f64::from(q.lon - p.lon);
        let dy = f64::from(q.lat - p.lat);
        let len2 = dx * dx + dy * dy;
        let mut furthest = None;
        let mut distance = tolerance * tolerance;
        for (i, r) in points.iter().enumerate().take(b).skip(a + 1) {
            let x = f64::from(r.lon - p.lon);
            let y = f64::from(r.lat - p.lat);
            let t = if len2 == 0.0 {
                0.0
            } else {
                ((x * dx + y * dy) / len2).clamp(0.0, 1.0)
            };
            let d = (x - t * dx).powi(2) + (y - t * dy).powi(2);
            if d > distance {
                distance = d;
                furthest = Some(i);
            }
        }
        if let Some(i) = furthest {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}
