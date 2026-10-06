use super::super::super::{contour_asset::residency, local_contour_pass::LocalTileId};
use super::*;

pub(super) fn tile(lon: f32, n: usize) -> LocalTileGeometry {
    let contours = Arc::new(vec![ContourPath {
        elevation_m: 200.,
        points: (0..n)
            .map(|i| GeoPoint {
                lon: lon + i as f32 * 0.0001,
                lat: 0.01 * (i as f32).sin(),
            })
            .collect(),
    }]);
    LocalTileGeometry {
        id: LocalTileId {
            zoom_bucket: 0,
            lat_bucket: 0,
            lon_bucket: lon as i32,
        },
        bounds: residency::Bounds::from_contours(&contours),
        contours,
    }
}
pub(super) fn frame(tiles: Vec<LocalTileGeometry>) -> Arc<Frame> {
    Arc::new(Frame { bytes: 0, tiles })
}
#[test]
fn spatial_batches_preserve_every_native_segment_and_reuse_neighbor_uploads() {
    let a = tile(0., 10000);
    let first = prepare(
        frame(vec![a.clone()]),
        None,
        egui::Color32::WHITE,
        egui::Color32::BLACK,
    );
    let next = prepare(
        frame(vec![a.clone(), tile(5., 10)]),
        Some(&first),
        egui::Color32::WHITE,
        egui::Color32::BLACK,
    );
    assert!(Arc::ptr_eq(&first.tiles[0], &next.tiles[0]));
    let mut actual: Vec<Vec<u8>> = first.tiles[0]
        .chunks
        .iter()
        .flat_map(|c| &c.instances)
        .map(|s| bytemuck::bytes_of(s).to_vec())
        .collect();
    let mut expected: Vec<Vec<u8>> = a.contours[0]
        .points
        .windows(2)
        .map(|p| {
            bytemuck::bytes_of(&SegmentInstance::line(p[0], p[1], egui::Color32::WHITE)).to_vec()
        })
        .collect();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
    assert!(first.tiles[0].chunks.len() > 1);
    let recolored = prepare(
        next.source.clone(),
        Some(&next),
        egui::Color32::RED,
        egui::Color32::BLACK,
    );
    assert!(!Arc::ptr_eq(&next.tiles[0], &recolored.tiles[0]));
}

#[test]
fn offscreen_batches_can_be_skipped_without_losing_crossing_segments() {
    let a = tile(0., 10);
    let prepared = prepare(
        frame(vec![a, tile(50., 10)]),
        None,
        egui::Color32::WHITE,
        egui::Color32::BLACK,
    );
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 400.));
    let layout = super::super::super::globe_scene::GlobeLayout {
        center: rect.center(),
        radius: 60000.,
        focal_length: 2.,
        camera_distance: 3.,
    };
    let view = GlobeViewState::from_focus(GeoPoint { lat: 0., lon: 0. });
    let viewport = Viewport::new(&layout, &view, rect);
    assert!(
        prepared.tiles[0]
            .chunks
            .iter()
            .all(|c| viewport.intersects(c.bounds))
    );
    assert!(
        prepared.tiles[1]
            .chunks
            .iter()
            .all(|c| !viewport.intersects(c.bounds))
    );
    let mut crossing = tile(-20., 2);
    Arc::make_mut(&mut crossing.contours)[0].points[1].lon = 20.;
    let batch = build_tile(&crossing, egui::Color32::WHITE, egui::Color32::BLACK);
    assert!(viewport.intersects(batch.chunks[0].bounds));
    assert!(batch.chunks[0].bounds.min_lon <= -20. && batch.chunks[0].bounds.max_lon >= 20.);
}
