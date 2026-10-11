use super::*;

fn bounds() -> Rect {
    Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 650.0))
}

fn request(id: &str, anchor: Pos2) -> Request<'_> {
    Request {
        id,
        anchor,
        size: egui::vec2(180.125, 85.375),
    }
}

#[test]
fn clustered_events_get_separate_cards_without_leaving_the_map() {
    let bounds = bounds();
    let mut occupied = Vec::new();
    for _ in 0..8 {
        let card = place_card(bounds.center(), egui::vec2(180.0, 85.0), bounds, &occupied);
        assert!(bounds.contains_rect(card));
        assert!(occupied.iter().all(|other| !other.intersects(card)));
        occupied.push(card);
    }
}

#[test]
fn edge_cards_remain_inside_the_viewport() {
    let bounds = bounds();
    for anchor in [
        bounds.left_top(),
        bounds.right_top(),
        bounds.left_bottom(),
        bounds.right_bottom(),
    ] {
        let card = place_card(anchor, egui::vec2(220.0, 100.0), bounds, &[]);
        assert!(bounds.contains_rect(card));
    }
}

#[test]
fn crowded_cards_hold_their_slots_through_an_entire_dwell_orbit() {
    let bounds = bounds();
    let mut layout = Layout::default();
    let ids: Vec<_> = (0..8).map(|index| format!("event-{index}")).collect();
    let mut initial = Vec::new();
    for frame in 0..600 {
        let now = frame as f64 / 60.0;
        layout.begin_frame(bounds, now);
        let requests: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let phase = now as f32 + i as f32 * 0.01;
                request(
                    id,
                    bounds.center() + egui::vec2(phase.sin() * 40.0, phase.cos() * 20.0),
                )
            })
            .collect();
        let cards = layout.place(&requests, now);
        if frame == 0 {
            initial = cards;
        } else {
            assert_eq!(cards, initial, "frame {frame}");
        }
    }
}

#[test]
fn new_and_temporarily_missing_events_do_not_displace_established_cards() {
    let bounds = bounds();
    let anchor = bounds.center();
    let mut layout = Layout::default();
    layout.begin_frame(bounds, 0.0);
    let initial = layout.place(&[request("b", anchor), request("c", anchor)], 0.0);
    layout.begin_frame(bounds, 0.1);
    // A new event sorts first, and C temporarily leaves the scene.
    let added = layout.place(&[request("a", anchor), request("b", anchor)], 0.1);
    assert_eq!(added[1], initial[0]);
    assert!(!added[0].intersects(initial[0]));
    assert!(!added[0].intersects(initial[1]));
    layout.begin_frame(bounds, 0.2);
    let restored = layout.place(&[request("c", anchor), request("b", anchor)], 0.2);
    assert_eq!(restored, vec![initial[1], initial[0]]);
}

#[test]
fn edge_hysteresis_does_not_admit_new_offscreen_events_and_stale_slots_expire() {
    let bounds = bounds();
    let viewport = bounds.expand(8.0);
    let mut layout = Layout::default();
    layout.begin_frame(bounds, 0.0);
    layout.place(&[request("existing", viewport.left_center())], 0.0);
    let just_outside = viewport.left_center() - egui::vec2(4.0, 0.0);
    assert!(layout.accepts_anchor("existing", just_outside, viewport));
    assert!(!layout.accepts_anchor("new", just_outside, viewport));
    assert!(!layout.accepts_anchor("existing", just_outside - egui::vec2(24.0, 0.0), viewport));
    layout.begin_frame(bounds, 3.0);
    assert!(!layout.accepts_anchor("existing", just_outside, viewport));
}

#[test]
fn viewport_and_content_size_changes_reflow_only_when_needed() {
    let mut layout = Layout::default();
    layout.begin_frame(bounds(), 0.0);
    let anchor = bounds().right_bottom();
    let initial = layout.place(&[request("a", anchor), request("b", anchor)], 0.0);
    let mut changed = request("a", anchor);
    changed.size.y += 35.0;
    layout.begin_frame(bounds(), 0.1);
    let resized = layout.place(&[changed, request("b", anchor)], 0.1);
    assert_eq!(resized[1], initial[1]);
    assert!(!resized[0].intersects(resized[1]));
    let small = Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 350.0));
    layout.begin_frame(small, 0.2);
    let reflowed = layout.place(
        &[request("a", small.center()), request("b", small.center())],
        0.2,
    );
    assert!(reflowed.iter().all(|rect| small.contains_rect(*rect)));
}
