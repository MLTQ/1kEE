//! Every captured transition frame must match one of two complete reference images.
use super::*;
use crate::model::GlobeViewState;
use crate::panels::world_map::{
    admin_globe, admin_globe_pass, contour_pass, globe_scene::GlobeLayout,
};

fn batch(low: f32, theme: MapTheme, padded: bool) -> Arc<admin_globe::Batch> {
    let ring: Vec<_> = tests::square(7, 4, low, low + 2.)
        .points
        .into_iter()
        .map(|p| GeoPoint {
            lon: p.lon,
            lat: p.lat,
        })
        .collect();
    let rings = vec![ring];
    let region = Region {
        relation_id: 7,
        level: 4,
        bounds: [low, low, low + 2., low + 2.],
        fill: Some(admin_geometry::tessellate(&rings).unwrap()),
        rings,
    };
    let mut batch = admin_globe::prepare(
        Arc::new(AdminData {
            regions: vec![region],
            ..Default::default()
        }),
        theme.admin_palette(),
    );
    if padded {
        // Invisible instances force both real uploads past the 4 MiB frame limit.
        let back = GeoPoint {
            lon: -178.,
            lat: 0.,
        };
        Arc::make_mut(&mut batch.lines).resize(
            180_000,
            contour_pass::SegmentInstance::line(back, back, egui::Color32::WHITE),
        );
        Arc::make_mut(&mut batch.triangles).resize(
            110_000,
            admin_globe_pass::Triangle::new([back; 3], egui::Color32::WHITE),
        );
    }
    Arc::new(batch)
}
#[test]
#[ignore = "requires GPU; checks every frame of a multi-frame upload"]
fn admin_globe_gpu_keeps_complete_boundaries_on_camera_handoff() {
    let mut gpu = screen::Screen::new([256, 256]);
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(256., 256.));
    let a = batch(0., MapTheme::Topo, false);
    let b = batch(4., MapTheme::Topo, false);
    let incoming = batch(4., MapTheme::Topo, true);
    let mut frame = |batch: Option<&Arc<admin_globe::Batch>>, moved: bool, shade: bool| {
        let layout = GlobeLayout {
            center: rect.center(),
            radius: if moved { 265. } else { 210. },
            focal_length: 2.2,
            camera_distance: 1.45,
        };
        let view = GlobeViewState::from_focus(if moved {
            GeoPoint { lon: 2., lat: 3. }
        } else {
            GeoPoint { lon: 3.5, lat: 2. }
        });
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx
                    .layer_painter(egui::LayerId::background())
                    .with_clip_rect(rect);
                painter.add(contour_pass::residency_callback(rect));
                if let Some(batch) = batch {
                    admin_globe::paint(&painter, &layout, &view, batch, shade, 0.3, 1.);
                }
            },
        );
        gpu.render(&ctx, out, true).unwrap()
    };
    let blank = frame(None, true, true);
    frame(Some(&b), false, true);
    let new_original = frame(Some(&b), false, true);
    let new = frame(Some(&b), true, true);
    frame(Some(&a), false, true);
    let original = frame(Some(&a), false, true);
    let old = frame(Some(&a), true, true);
    assert!(old != original && old != new && old != blank && new != blank);
    let lines_only = frame(Some(&a), true, false);
    assert!(lines_only != old && lines_only != blank);
    frame(Some(&a), true, true);
    let mut previous_frames = 0;
    for step in 0..120 {
        let moved = step % 2 == 0;
        let picture = frame(Some(&incoming), moved, true);
        let (before, after) = if moved {
            (&old, &new)
        } else {
            (&original, &new_original)
        };
        if &picture == after {
            break;
        }
        assert!(
            &picture == before,
            "partly uploaded, blank or incorrectly reprojected frame"
        );
        previous_frames += 1;
    }
    assert!(
        (2..120).contains(&previous_frames),
        "replacement must span frames and finish"
    );
    let empty = Arc::new(admin_globe::prepare(
        Arc::new(AdminData::default()),
        MapTheme::Topo.admin_palette(),
    ));
    assert!(
        frame(Some(&empty), true, true) == blank,
        "a ready empty view retires old boundaries"
    );
    frame(Some(&b), true, true);
    assert!(frame(Some(&b), true, true) == new);
    // A different palette must never borrow the preceding theme while it uploads.
    let changed = batch(0., MapTheme::Sodium, true);
    assert!(frame(Some(&changed), true, true) == blank);
    // Hiding mid-upload retires the pending pair and leaves no old buffers drawable.
    assert!(frame(None, true, true) == blank);
    assert!(frame(Some(&incoming), true, true) == blank);
}
