//! Screen-density LOD selects complete elevation planes, never path vertices.
use std::ops::Range;

const TARGET_SEGMENTS: usize = 4_000_000;
const RESTORE_SEGMENTS: usize = 2_800_000;
const MAX_INTERVAL_M: i32 = 12_800;

pub(super) struct Plane {
    pub elevation: i32,
    pub range: Range<u32>,
}
pub(super) struct Selection {
    pub interval_m: i32,
}
impl Default for Selection {
    fn default() -> Self {
        Self { interval_m: 1 }
    }
}
fn lower(interval: i32) -> i32 {
    if interval <= 100 { 1 } else { interval / 2 }
}
impl Selection {
    pub fn update(&mut self, count: impl Fn(i32) -> usize) {
        while count(self.interval_m) > TARGET_SEGMENTS && self.interval_m < MAX_INTERVAL_M {
            self.interval_m = if self.interval_m == 1 {
                100
            } else {
                self.interval_m * 2
            };
        }
        // Hysteresis avoids toggling planes at a viewport/tile boundary.
        while self.interval_m > 1 && count(lower(self.interval_m)) < RESTORE_SEGMENTS {
            self.interval_m = lower(self.interval_m);
        }
    }
}
pub(super) fn selected(elevation: i32, interval: i32) -> bool {
    interval == 1 || elevation.rem_euclid(interval) == 0
}
pub(super) fn count(planes: &[Plane], interval: i32) -> usize {
    planes
        .iter()
        .filter(|p| selected(p.elevation, interval))
        .map(|p| (p.range.end - p.range.start) as usize)
        .sum()
}
/// Combine adjacent selected runs, keeping draw overhead low at every density.
pub(super) fn ranges(
    planes: &[Plane],
    count: u32,
    interval: i32,
    mut draw: impl FnMut(Range<u32>),
) {
    if interval == 1 {
        if count > 0 {
            draw(0..count);
        }
        return;
    }
    let mut pending: Option<Range<u32>> = None;
    for plane in planes.iter().filter(|p| selected(p.elevation, interval)) {
        if let Some(range) = &mut pending
            && range.end == plane.range.start
        {
            range.end = plane.range.end;
        } else {
            if let Some(range) = pending.take() {
                draw(range);
            }
            pending = Some(plane.range.clone());
        }
    }
    if let Some(range) = pending {
        draw(range);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn density_omits_whole_planes_and_restores_full_geometry_with_hysteresis() {
        let planes: Vec<_> = (0..16)
            .map(|i| Plane {
                elevation: i * 100,
                range: i as u32 * 1_000_000..(i + 1) as u32 * 1_000_000,
            })
            .collect();
        let mut selection = Selection::default();
        selection.update(|i| count(&planes, i));
        assert_eq!(selection.interval_m, 400);
        let mut kept = Vec::new();
        ranges(&planes, 16_000_000, selection.interval_m, |r| kept.push(r));
        assert_eq!(
            kept,
            vec![
                0..1_000_000,
                4_000_000..5_000_000,
                8_000_000..9_000_000,
                12_000_000..13_000_000
            ]
        );
        selection.update(|i| count(&planes, i) / 2); // next finer is still over restore threshold
        assert_eq!(selection.interval_m, 400);
        selection.update(|i| count(&planes, i) / 8);
        assert_eq!(selection.interval_m, 1);
        let mut all = Vec::new();
        ranges(&planes, 16_000_000, 1, |r| all.push(r));
        assert_eq!(all, vec![0..16_000_000]);
        assert!(selected(-800, 400));
        assert!(!selected(-600, 400));
    }
    #[test]
    fn adjacent_selected_planes_share_a_draw_and_zero_elevation_never_disappears() {
        let planes = vec![
            Plane {
                elevation: 0,
                range: 0..2,
            },
            Plane {
                elevation: 400,
                range: 2..7,
            },
            Plane {
                elevation: 600,
                range: 7..8,
            },
        ];
        let mut kept = Vec::new();
        ranges(&planes, 8, 400, |r| kept.push(r));
        assert_eq!(kept, vec![0..7]);
        let mut selection = Selection::default();
        selection.update(|_| 10_000_000);
        assert_eq!(selection.interval_m, MAX_INTERVAL_M);
        assert!(selected(0, selection.interval_m));
    }
}
