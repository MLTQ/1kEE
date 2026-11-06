use cell_format::CellPoint;

/// Maximum perpendicular deviation in unwrapped geographic degrees. Full detail
/// references the original coordinates. Each coarser tier is a subset of the finer one.
pub const ERRORS: [f32; 7] = [0.15, 0.04, 0.01, 0.0025, 0.0005, 0.0001, 0.0];
pub const FULL: usize = ERRORS.len() - 1;

pub fn tier_for_scale(degrees_per_pixel: f32) -> usize {
    ERRORS
        .iter()
        .position(|e| *e <= degrees_per_pixel * 0.75)
        .unwrap_or(FULL)
}

pub fn unwrap(points: &mut [CellPoint], mut previous: f32) {
    for p in points {
        p.lon += ((previous - p.lon) / 360.).round() * 360.;
        previous = p.lon;
    }
}

fn distance_squared(p: CellPoint, a: CellPoint, b: CellPoint) -> f64 {
    let (x, y) = (
        f64::from(p.lon) - f64::from(a.lon),
        f64::from(p.lat) - f64::from(a.lat),
    );
    let (dx, dy) = (
        f64::from(b.lon) - f64::from(a.lon),
        f64::from(b.lat) - f64::from(a.lat),
    );
    let t = ((x * dx + y * dy) / (dx * dx + dy * dy)).clamp(0., 1.);
    let t = if t.is_nan() { 0. } else { t };
    (x - t * dx).powi(2) + (y - t * dy).powi(2)
}

/// Mark source vertices once for all levels using a bounded-error Douglas–Peucker
/// hierarchy. Three separated anchors keep even subpixel closed rings intact.
pub fn importance(points: &[CellPoint]) -> Vec<f64> {
    if points.len() <= 2 {
        return vec![f64::INFINITY; points.len()];
    }
    let end = points.len() - 1;
    let mut weights = vec![0.; points.len()];
    let mut anchors = vec![0, end];
    if end >= 3 && points[0] == points[end] {
        let far = (1..end)
            .max_by_key(|&i| distance_squared(points[i], points[0], points[0]).to_bits())
            .unwrap();
        let side = (1..end)
            .filter(|&i| i != far)
            .max_by_key(|&i| distance_squared(points[i], points[0], points[far]).to_bits())
            .unwrap();
        anchors.extend([far, side]);
        anchors.sort_unstable();
    }
    for &i in &anchors {
        weights[i] = f64::INFINITY;
    }
    let minimum = f64::from(ERRORS[FULL - 1]).powi(2);
    let mut stack: Vec<_> = anchors
        .windows(2)
        .map(|pair| (pair[0], pair[1], f64::INFINITY))
        .collect();
    while let Some((a, b, parent)) = stack.pop() {
        let mut error = 0.;
        let mut split = a;
        for i in a + 1..b {
            let d = distance_squared(points[i], points[a], points[b]);
            if d > error {
                error = d;
                split = i;
            }
        }
        let effective = parent.min(error);
        if effective >= minimum {
            weights[split] = effective;
            stack.extend([(a, split, effective), (split, b, effective)]);
        }
    }
    weights
}

pub fn simplify(points: &[CellPoint], weights: &[f64], tier: usize) -> Vec<CellPoint> {
    let threshold = f64::from(ERRORS[tier]).powi(2);
    points
        .iter()
        .zip(weights)
        .filter(|(_, w)| **w >= threshold)
        .map(|(&p, _)| p)
        .collect()
}

pub fn bounds(points: &[CellPoint]) -> [f32; 4] {
    let mut b = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for p in points {
        b[0] = b[0].min(p.lon);
        b[1] = b[1].min(p.lat);
        b[2] = b[2].max(p.lon);
        b[3] = b[3].max(p.lat);
    }
    b
}
