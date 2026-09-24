//! Nonoverlapping ownership on the existing Earth contour address grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
}
#[derive(Clone, Copy, Debug)]
pub struct CoreTile {
    pub core: Bounds,
    pub source: Bounds,
    pub raster_size: u32,
}
impl CoreTile {
    /// Keep old addresses and sample spacing, but discard the overlapping footprint.
    pub fn new(half_extent: f32, old_raster_size: u32, y: i32, x: i32) -> Self {
        let step = f64::from(half_extent * 0.45);
        let core = Bounds {
            min_lon: (f64::from(x) - 0.5) * step,
            max_lon: (f64::from(x) + 0.5) * step,
            min_lat: (f64::from(y) - 0.5) * step,
            max_lat: (f64::from(y) + 0.5) * step,
        };
        let pixels = (f64::from(old_raster_size) * 0.225).ceil().max(2.0) as u32;
        // Neighboring samples are needed by interpolation and marching squares.
        let halo = step / f64::from(pixels) * 2.0;
        Self {
            core,
            source: Bounds {
                min_lon: core.min_lon - halo,
                max_lon: core.max_lon + halo,
                min_lat: core.min_lat - halo,
                max_lat: core.max_lat + halo,
            },
            raster_size: pixels + 4,
        }
    }
}
/// Clip edges, insert boundary intersections, and split at exits. North/east
/// boundary-aligned edges belong to the neighbor; crossings keep both endpoints.
pub fn clip_line(points: &[(f64, f64)], b: Bounds) -> Vec<Vec<(f64, f64)>> {
    let mut lines = Vec::new();
    let mut current = Vec::new();
    for pair in points.windows(2) {
        if let Some((a, z)) = clip_edge(pair[0], pair[1], b) {
            if current.last().is_some_and(|last| *last != a) {
                finish(&mut current, &mut lines);
            }
            if current.is_empty() {
                current.push(a);
            }
            current.push(z);
        } else {
            finish(&mut current, &mut lines);
        }
    }
    finish(&mut current, &mut lines);
    lines
}
fn finish(line: &mut Vec<(f64, f64)>, lines: &mut Vec<Vec<(f64, f64)>>) {
    if line.len() >= 2 {
        lines.push(std::mem::take(line));
    } else {
        line.clear();
    }
}
fn clip_edge(a: (f64, f64), z: (f64, f64), b: Bounds) -> Option<((f64, f64), (f64, f64))> {
    if ![a.0, a.1, z.0, z.1].iter().all(|v| v.is_finite()) {
        return None;
    }
    if (a.0 == b.max_lon && z.0 == b.max_lon) || (a.1 == b.max_lat && z.1 == b.max_lat) {
        return None;
    }
    let dx = z.0 - a.0;
    let dy = z.1 - a.1;
    let mut lo: f64 = 0.0;
    let mut hi: f64 = 1.0;
    for (p, q) in [
        (-dx, a.0 - b.min_lon),
        (dx, b.max_lon - a.0),
        (-dy, a.1 - b.min_lat),
        (dy, b.max_lat - a.1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else if p < 0.0 {
            lo = lo.max(q / p);
        } else {
            hi = hi.min(q / p);
        }
        if lo >= hi {
            return None;
        }
    }
    let at = |t: f64| {
        (
            (a.0 + t * dx).clamp(b.min_lon, b.max_lon),
            (a.1 + t * dy).clamp(b.min_lat, b.max_lat),
        )
    };
    let result = (at(lo), at(hi));
    (result.0 != result.1).then_some(result)
}
#[cfg(test)]
#[path = "contour_grid_tests.rs"]
mod tests;
