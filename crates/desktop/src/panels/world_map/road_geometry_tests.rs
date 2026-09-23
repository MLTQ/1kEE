use super::*;

fn endpoint(i: usize) -> (GeoPoint, f32) {
    (
        GeoPoint {
            lat: i as f32 * 0.00001,
            lon: 2.0,
        },
        i as f32,
    )
}

fn endpoints(instance: &LocalSegmentInstance) -> ([f32; 3], [f32; 3]) {
    let values: &[f32] = bytemuck::cast_slice(std::slice::from_ref(instance));
    (
        values[0..3].try_into().unwrap(),
        values[3..6].try_into().unwrap(),
    )
}

#[test]
fn upload_boundaries_preserve_every_segment_and_never_join_separate_ways() {
    let mut builder = BatchBuilder::new(true, 7, egui::Color32::YELLOW);
    let points: Vec<_> = (0..SEGMENTS_PER_BATCH + 3).map(endpoint).collect();
    builder.push_line(&points);
    builder.push_line(&[endpoint(2_000_000), endpoint(2_000_001)]);
    let batches = builder.finish();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].instances.len(), SEGMENTS_PER_BATCH);
    assert_eq!(batches[1].instances.len(), 3);
    let first_end = endpoints(batches[0].instances.last().unwrap()).1;
    let next_start = endpoints(&batches[1].instances[0]).0;
    assert_eq!(first_end, next_start);
    assert_eq!(endpoints(&batches[1].instances[2]).0[2], 2_000_000.0);
    for batch in batches {
        assert!(std::mem::size_of_val(&batch.instances[..]) <= 2 * 1024 * 1024);
    }
}

#[test]
fn short_connectors_survive_after_the_old_major_point_budget_is_exhausted() {
    let roads = (0..2_200).map(|id| {
        let count = if id < 2_199 { 192 } else { 2 };
        RoadPolyline {
            way_id: id,
            road_class: "primary".into(),
            name: None,
            points: (0..count).map(|i| endpoint(i).0).collect(),
            elevations: Some(vec![123.0; count]),
        }
    });
    let geometry = RoadGeometry::build(roads, None, 3, [egui::Color32::YELLOW; 2]);
    assert!(geometry.minor.is_empty());
    assert_eq!(
        geometry
            .major
            .iter()
            .map(|b| b.instances.len())
            .sum::<usize>(),
        2_199 * 191 + 1
    );
    let connector = geometry.major.last().unwrap().instances.last().unwrap();
    assert_eq!(
        endpoints(connector),
        ([2.0, 0.0, 126.0], [2.0, 0.00001, 126.0])
    );
}

#[test]
fn invalid_coordinates_do_not_create_bridges() {
    let mut builder = BatchBuilder::new(false, 9, egui::Color32::WHITE);
    let mut points: Vec<_> = (0..5).map(endpoint).collect();
    points[2].0.lat = f32::NAN;
    builder.push_line(&points);
    let batches = builder.finish();
    assert_eq!(batches[0].instances.len(), 2);
    assert_eq!(endpoints(&batches[0].instances[1]).0[2], 3.0);
}

#[test]
#[ignore = "read-only regional benchmark; requires ONEKEE_ROAD_BENCH_CELL"]
fn benchmark_real_road_frame_preparation() {
    use crate::model::GlobeViewState;
    use crate::panels::world_map::local_contour_pass::{LocalContourCallback, LocalContourPass};
    use crate::panels::world_map::local_terrain_scene::{LocalLayout, project_local, projection};
    use std::time::Instant;
    let path = std::env::var("ONEKEE_ROAD_BENCH_CELL").expect("explicit cell path");
    let bytes = std::fs::read(&path).unwrap();
    let features = cell_format::read::read_single_chunk(&bytes, cell_format::TAG_ROAD).unwrap();
    let roads: Vec<_> = features
        .into_iter()
        .map(|f| RoadPolyline {
            way_id: f.way_id,
            road_class: cell_format::decode_road_class(f.class).into(),
            name: f.name,
            elevations: Some(vec![0.0; f.points.len()]),
            points: f
                .points
                .into_iter()
                .map(|p| GeoPoint {
                    lat: p.lat,
                    lon: p.lon,
                })
                .collect(),
        })
        .collect();
    let focus = roads
        .iter()
        .find_map(|r| r.points.first())
        .copied()
        .unwrap();
    let mut old_major = Vec::new();
    let mut old_minor = Vec::new();
    for road in &roads {
        let pts = crate::feature_heights::prepare(
            &road.points,
            road.elevations.as_deref(),
            192,
            3.0,
            |_| unreachable!(),
        );
        if matches!(
            road.road_class.as_str(),
            "motorway" | "trunk" | "primary" | "secondary"
        ) {
            old_major.push(pts);
        } else {
            old_minor.push(pts);
        }
    }
    old_major.sort_by_key(|p| std::cmp::Reverse(p.len()));
    old_minor.sort_by_key(|p| std::cmp::Reverse(p.len()));
    for (name, lines, budget) in [
        ("major", &old_major, 400_000usize),
        ("minor", &old_minor, 800_000),
    ] {
        let mut remaining = budget;
        let mut omitted = 0;
        for line in lines {
            if remaining < line.len() {
                omitted += 1;
            }
            remaining = remaining.saturating_sub(line.len());
        }
        eprintln!(
            "{name}: {} ways, {} prepared points; old budget omitted/truncated {omitted} ways",
            lines.len(),
            lines.iter().map(Vec::len).sum::<usize>()
        );
    }
    let started = Instant::now();
    let geometry = RoadGeometry::build(roads, None, 1, [egui::Color32::YELLOW; 2]);
    let batches: Vec<_> = geometry.major.into_iter().chain(geometry.minor).collect();
    eprintln!(
        "background instance build {:?}; {} batches, {} segments, {} bytes",
        started.elapsed(),
        batches.len(),
        batches.iter().map(|b| b.instances.len()).sum::<usize>(),
        batches
            .iter()
            .map(|b| std::mem::size_of_val(&b.instances[..]))
            .sum::<usize>()
    );
    let layout = LocalLayout {
        center: egui::pos2(512.0, 384.0),
        focus_center: egui::pos2(512.0, 384.0),
        width: 1024.0,
        height: 768.0,
        horizontal_scale: 512.0,
    };
    let mut view = GlobeViewState::from_focus(focus);
    view.local_yaw = 0.0;
    view.local_pitch = 0.0;
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0));
    let params = projection::local_projection_params(&layout, &view, focus, 100.0, 100.0);
    for _ in 0..3 {
        for gpu in [false, true] {
            let start = Instant::now();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Foreground,
                        egui::Id::new("bench"),
                    ));
                    if gpu {
                        painter.add(
                            LocalContourCallback::new(
                                LocalContourPass::Roads,
                                batches.clone(),
                                &params,
                                1.0,
                                0.8,
                                1.35,
                                ctx.clone(),
                            )
                            .into_paint_callback(rect),
                        );
                    } else {
                        for (lines, mut budget, width) in
                            [(&old_major, 400_000usize, 1.35), (&old_minor, 800_000, 0.8)]
                        {
                            for line in lines {
                                if budget < 2 {
                                    break;
                                }
                                let mut points = Vec::new();
                                for &(point, height) in line {
                                    if budget == 0 {
                                        break;
                                    }
                                    if let Some(p) = project_local(
                                        &layout, &view, focus, point, height, 100.0, 100.0,
                                    ) {
                                        points.push(p.pos);
                                        budget -= 1;
                                    }
                                }
                                if points.len() >= 2 {
                                    painter.add(egui::Shape::line(
                                        points,
                                        egui::Stroke::new(width, egui::Color32::YELLOW),
                                    ));
                                }
                            }
                        }
                    }
                },
            );
            let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
            eprintln!(
                "{} UI preparation {:?}, {} paint jobs",
                if gpu { "retained GPU" } else { "old CPU" },
                start.elapsed(),
                jobs.len()
            );
            std::hint::black_box(jobs);
        }
    }
}
