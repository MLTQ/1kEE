//! Select source detail by its ground footprint, independently of camera scale.
use super::super::srtm_focus_cache::zoom;
use crate::model::ActiveBody;

pub(super) const ZOOMS: [f32; 11] = [0.5, 1.5, 2.5, 3.5, 5.0, 8.0, 12.0, 16.0, 25.0, 35.0, 50.0];

#[derive(Clone, Copy, Debug)]
pub(super) struct Selection {
    pub zoom: f32,
    pub radius: i32,
}

pub(super) fn step(body: ActiveBody, source_zoom: f32) -> f32 {
    let spec = match body {
        ActiveBody::Earth => zoom::spec_for_zoom(source_zoom),
        ActiveBody::Moon => zoom::lunar_spec_for_zoom(source_zoom),
        ActiveBody::Mars => zoom::mars_spec_for_zoom(source_zoom),
    };
    spec.half_extent_deg * 0.45
}

pub(super) fn select(body: ActiveBody, camera_zoom: f32, previous: Option<Selection>) -> Selection {
    let half =
        super::visual_half_extent_for_zoom(camera_zoom) * zoom::OBLIQUE_VISIBLE_EXTENT_FACTOR;
    let nominal = match body {
        ActiveBody::Earth => zoom::spec_for_zoom(camera_zoom).zoom_bucket,
        _ => zoom::lunar_spec_for_zoom(camera_zoom).zoom_bucket,
    } as usize;
    // Four rings (81 tiles) is the normal budget. Allow a fifth ring on an
    // already selected tier so a tiny reverse zoom doesn't switch it back.
    let radius = |z| (half / step(body, z)).ceil().max(1.0) as i32;
    let desired = (0..=nominal)
        .rev()
        .find(|&i| radius(ZOOMS[i]) <= 4)
        .unwrap_or(0);
    let mut zoom = ZOOMS[desired];
    if let Some(old) = previous
        && old.zoom > zoom
        && radius(old.zoom) <= 5
    {
        zoom = old.zoom;
    }
    // The widest possible camera needs seven rings of the coarsest existing
    // tier (225 tiles). Never crop coverage just to meet the normal budget.
    Selection {
        zoom,
        radius: radius(zoom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_arrival_uses_81_tiles_instead_of_529() {
        let s = select(ActiveBody::Earth, 9.5, None);
        assert_eq!(s.zoom, 5.0);
        assert_eq!(s.radius, 4);
    }

    #[test]
    fn every_zoom_preserves_coverage_with_bounded_requests_in_both_directions() {
        for body in [ActiveBody::Earth, ActiveBody::Moon, ActiveBody::Mars] {
            for reverse in [false, true] {
                let mut previous = None;
                for n in 0..=5900 {
                    let camera = 1.0 + (if reverse { 5900 - n } else { n }) as f32 * 0.01;
                    let s = select(body, camera, previous);
                    assert!(s.radius <= 7, "{body:?} {camera}: {s:?}");
                    assert!(s.zoom == 0.5 || s.radius <= 5);
                    assert!(
                        s.radius as f32 * step(body, s.zoom) + 1e-6
                            >= super::super::visual_half_extent_for_zoom(camera) * 2.5
                    );
                    previous = Some(s);
                }
            }
        }
    }

    #[test]
    fn small_reverse_zoom_keeps_the_selected_tier() {
        let s = select(ActiveBody::Earth, 9.5, None);
        assert_eq!(select(ActiveBody::Earth, 9.4, Some(s)).zoom, s.zoom);
        assert_eq!(select(ActiveBody::Earth, 9.6, Some(s)).zoom, s.zoom);
        // Crossing a nominal source threshold is not a reason to throw out a
        // fine tier that still fits the coverage budget.
        let fine = select(ActiveBody::Earth, 13.0, None);
        assert_eq!(select(ActiveBody::Earth, 12.99, Some(fine)).zoom, fine.zoom);
    }
}
