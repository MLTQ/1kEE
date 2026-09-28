use super::*;
use crate::model::GeoPoint;
use std::collections::BTreeSet;

fn segment(a: [f32; 2], b: [f32; 2]) -> SegmentInstance {
    SegmentInstance::line(
        GeoPoint {
            lat: a[0],
            lon: a[1],
        },
        GeoPoint {
            lat: b[0],
            lon: b[1],
        },
        egui::Color32::WHITE,
    )
}
fn layout() -> GlobeLayout {
    GlobeLayout {
        center: egui::pos2(600.0, 400.0),
        radius: 400.0,
        focal_length: 2.0,
        camera_distance: 2.5,
    }
}

#[test]
fn index_matches_brute_force_across_camera_changes_and_overlaps() {
    let mut segments = Vec::new();
    for lat in (-80..=80).step_by(8) {
        for lon in (-180..180).step_by(8) {
            segments.push(segment(
                [lat as f32, lon as f32],
                [lat as f32 + 3.0, lon as f32 + 5.0],
            ));
        }
    }
    segments.push(segment([0.0, 179.0], [0.0, -179.0]));
    segments.push(segment([0.0, 80.0], [0.0, 100.0]));
    segments.push(segment([0.0, 80.0], [0.0, 100.0])); // independent overlapping records
    let index = Index::new(&segments, (0..segments.len()).collect());
    for (yaw, pitch, distance) in [(0.0, 0.0, 2.5), (1.1, 0.7, 1.2), (-2.3, -0.6, 4.0)] {
        let mut layout = layout();
        layout.camera_distance = distance;
        let mut view = GlobeViewState::from_focus(GeoPoint {
            lat: 0.0,
            lon: 90.0,
        });
        view.yaw = yaw;
        view.pitch = pitch;
        let project = Projection::new(&layout, &view);
        for segment in segments.iter().step_by(7) {
            let [a, b] = segment.endpoints();
            let (Some(a), Some(b)) = (project.point(a), project.point(b)) else {
                continue;
            };
            let pointer = a + (b - a) * 0.43 + egui::vec2(0.0, 2.0);
            let mut actual = BTreeSet::new();
            index.query(&segments, &layout, &view, pointer, |i, _| {
                actual.insert(i);
            });
            let expected: BTreeSet<_> = segments
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    let [a, b] = s.endpoints();
                    let (a, b) = (project.point(a)?, project.point(b)?);
                    (infrastructure_hover::line_distance(pointer, a, b)
                        <= infrastructure_hover::RADIUS)
                        .then_some(i)
                })
                .collect();
            assert_eq!(
                actual, expected,
                "camera {yaw}/{pitch}, pointer {pointer:?}"
            );
        }
    }
}

#[test]
fn horizon_and_hidden_endpoint_match_gpu_segment_culling() {
    let layout = layout();
    let view = GlobeViewState::from_focus(GeoPoint {
        lat: 0.0,
        lon: 90.0,
    });
    let project = Projection::new(&layout, &view);
    assert_eq!(project.point([0.0, 0.0, 1.0]), Some(layout.center));
    assert!(project.point([0.0, 0.0, -1.0]).is_none());
    assert!(project.point([0.9165, 0.0, 0.399]).is_none());
    let segments = vec![segment([0.0, 90.0], [0.0, -90.0])];
    let index = Index::new(&segments, vec![0]);
    let mut hits = Vec::new();
    index.query(&segments, &layout, &view, layout.center, |i, _| {
        hits.push(i)
    });
    assert!(hits.is_empty());
}

#[test]
#[ignore = "requires installed pipeline archive"]
fn real_pipeline_hover_index() {
    let path = std::env::var("ONEKEE_PIPELINE_ARCHIVE").unwrap();
    let reader = tile_archive::Reader::open(std::path::Path::new(&path)).unwrap();
    let features = tile_archive::pipelines::load(&reader, None).unwrap();
    let mut segments = Vec::new();
    let mut owners = Vec::new();
    for (owner, f) in features
        .iter()
        .enumerate()
        .filter(|(_, f)| tile_archive::pipelines::Filter::default().accepts(&f.info))
    {
        for p in f.points.windows(2).filter(|p| p[0] != p[1]) {
            segments.push(segment([p[0].lat, p[0].lon], [p[1].lat, p[1].lon]));
            owners.push(owner);
        }
    }
    let start = std::time::Instant::now();
    let index = Index::new(&segments, owners.clone());
    eprintln!(
        "Built hover BVH for {} segments in {:?}",
        segments.len(),
        start.elapsed()
    );
    let layout = layout();
    let mut view = GlobeViewState::from_focus(GeoPoint {
        lat: 0.0,
        lon: 90.0,
    });
    view.yaw = (-34.0f32).to_radians();
    view.pitch = 26.0f32.to_radians();
    let p = Projection::new(&layout, &view);
    let target = segments
        .iter()
        .enumerate()
        .find(|(i, _)| features[owners[*i]].info.source_id == "P3951")
        .unwrap();
    let [a, b] = target.1.endpoints();
    let a = p.point(a).unwrap();
    let b = p.point(b).unwrap();
    let mut found = false;
    index.query(&segments, &layout, &view, a + (b - a) * 0.5, |owner, _| {
        found |= features[owner].info.source_id == "P3951";
    });
    assert!(
        found,
        "Siri–Mobarak pipeline should be hoverable in the Gulf"
    );
    let start = std::time::Instant::now();
    let mut tested = 0;
    for y in 0..10 {
        for x in 0..16 {
            tested += index.query(
                &segments,
                &layout,
                &view,
                egui::pos2(x as f32 * 80.0, y as f32 * 80.0),
                |_, _| {},
            );
        }
    }
    eprintln!(
        "160 hover queries: {:?}, {:.1} exact segment tests/query ({} total segments)",
        start.elapsed(),
        tested as f64 / 160.0,
        segments.len()
    );
    assert!(
        tested / 160 < segments.len() / 10,
        "hover must not scan the whole scene"
    );
}
