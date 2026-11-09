//! A padded viewport on an integer geographic grid. Pixel-aligned requests
//! share exact edges on either side of the antimeridian.
use super::{Product, provider};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Window {
    pub columns: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Window {
    pub fn plan(
        product: Product,
        visible: [f32; 4],
        display_pixels: f32,
        max_texture: usize,
    ) -> Option<Self> {
        let [west, south, east, north] = visible;
        let south = south.max(-90.);
        let north = north.min(90.);
        if !visible.iter().all(|v| v.is_finite())
            || west >= east
            || south >= north
            || east - west > 360.01
        {
            return None;
        }
        let limit = max_texture.min(2048) as i32;
        let budget = (display_pixels * 1.5).ceil().min(limit as f32);
        let span = (east - west).max(north - south);
        // GIBS 250m, level 8: 320 tiles of 512 pixels wide. The IR mosaic
        // has a coarser source grid. Avoid enlarging small provider pixels.
        let mut columns = match product {
            Product::TrueColor => 163840,
            Product::Infrared => 16384,
        };
        while columns > provider::size(product)[0] as i32 {
            let scale = columns as f64 / 360.;
            // Retain nearby views and prefetch a little beyond the visible edge.
            // Snap to blocks so subpixel camera movement doesn't start new work.
            let margin = f64::from(span) * 0.12;
            let lo = |deg: f64| (((deg - margin) * scale / 32.).floor() as i32) * 32;
            let hi = |deg: f64| (((deg + margin) * scale / 32.).ceil() as i32) * 32;
            let x = lo(f64::from(west) + 180.);
            let y = lo(f64::from(south) + 90.).max(0);
            let width = (hi(f64::from(east) + 180.) - x).min(columns);
            let height = hi(f64::from(north) + 90.).min(columns / 2) - y;
            if width.max(height) <= limit && width.max(height) as f32 <= budget {
                return Some(Self {
                    columns,
                    x: x.rem_euclid(columns),
                    y,
                    width,
                    height,
                });
            }
            columns /= 2;
        }
        None
    }

    fn edges(self, x: i32, width: i32) -> [f64; 4] {
        let step = 360. / f64::from(self.columns);
        [
            f64::from(x) * step - 180.,
            f64::from(self.y) * step - 90.,
            f64::from(x + width) * step - 180.,
            f64::from(self.y + self.height) * step - 90.,
        ]
    }

    pub fn bounds(self) -> [f32; 4] {
        self.edges(self.x, self.width).map(|v| v as f32)
    }

    pub fn contains(self, [mut west, south, mut east, north]: [f32; 4]) -> bool {
        let [l, b, r, t] = self.bounds();
        let shift = ((l + r - west - east) / 720.).round() * 360.;
        west += shift;
        east += shift;
        west >= l && east <= r && south.max(-90.) >= b && north.min(90.) <= t
    }

    pub fn size(self) -> [u32; 2] {
        [self.width as u32, self.height as u32]
    }

    pub fn segments(self) -> impl Iterator<Item = (i32, [f64; 4], [u32; 2])> {
        let first = self.width.min(self.columns - self.x);
        [(0, self.x, first), (first, 0, self.width - first)]
            .into_iter()
            .filter(|&(_, _, w)| w > 0)
            .map(move |(offset, x, w)| (offset, self.edges(x, w), [w as u32, self.height as u32]))
    }
}
