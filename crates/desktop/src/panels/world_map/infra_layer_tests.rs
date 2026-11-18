use super::*;

fn feature(class: &str, polygon: bool, coordinates: &[(f32, f32)]) -> LoadedPolyline {
    LoadedPolyline {
        way_id: 1,
        class: class.into(),
        name: Some("fixture".into()),
        points: coordinates
            .iter()
            .map(|&(lon, lat)| GeoPoint { lon, lat })
            .collect(),
        elevations: None,
        is_polygon: polygon,
    }
}
fn shapes(feature: &InfraFeature) -> Vec<egui::Shape> {
    feature.shapes(
        egui::Rect::EVERYTHING,
        aeroway_stroke(&feature.class),
        aeroway_fill(&feature.class),
        |p, _| Some(egui::pos2(p.lon, p.lat)),
    )
}

#[test]
fn airports_show_perimeters_separately_from_actual_runway_strips() {
    let boundary = feature(
        "dom_airport",
        true,
        &[(0., 0.), (4., 0.), (4., 4.), (3., 1.), (0., 4.), (0., 0.)],
    );
    let boundary = InfraFeature::prepare(boundary, "aeroway");
    assert!(boundary.fill.is_none());
    let paths = shapes(&boundary);
    assert_eq!(paths.len(), 1);
    let egui::Shape::Path(path) = &paths[0] else {
        panic!("perimeter should be an outline")
    };
    assert_eq!(path.fill, egui::Color32::TRANSPARENT);
    assert_eq!(path.points.first(), path.points.last());
    let runway =
        InfraFeature::prepare(feature("runway", false, &[(1., 0.5), (1., 3.5)]), "aeroway");
    assert!(runway.fill.is_none());
    let strips = shapes(&runway);
    let egui::Shape::Path(strip) = &strips[0] else {
        panic!("open runway stays a line")
    };
    assert_eq!(strip.points.len(), 2);
    assert!(strip.stroke.width > path.stroke.width);
    assert_ne!(strip.points.first(), strip.points.last());
}

#[test]
fn concave_terminal_and_infrastructure_meshes_leave_their_courtyards_empty() {
    let ring = &[
        (0., 0.),
        (4., 0.),
        (4., 4.),
        (3., 4.),
        (3., 1.),
        (1., 1.),
        (1., 4.),
        (0., 4.),
        (0., 0.),
    ];
    for (prefix, class) in [("aeroway", "terminal"), ("government", "government")] {
        let prepared = InfraFeature::prepare(feature(class, true, ring), prefix);
        let mesh = geometry::project_fill(
            prepared.fill.as_ref().unwrap(),
            egui::Color32::WHITE,
            egui::Rect::EVERYTHING,
            |p, _| Some(egui::pos2(p.lon, p.lat)),
        );
        let area: f32 = mesh
            .indices
            .chunks_exact(3)
            .map(|triangle| {
                let [a, b, c] =
                    [triangle[0], triangle[1], triangle[2]].map(|i| mesh.vertices[i as usize].pos);
                let centroid = (a.to_vec2() + b.to_vec2() + c.to_vec2()) / 3.;
                assert!(
                    !(centroid.x > 1. && centroid.x < 3. && centroid.y > 1.),
                    "no fan across courtyard"
                );
                let ab = b - a;
                let ac = c - a;
                (ab.x * ac.y - ab.y * ac.x).abs() / 2.
            })
            .sum();
        assert!((area - 10.).abs() < 0.0001);
    }
}

#[test]
fn old_runway_cell_flags_do_not_hide_closed_source_footprints() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("aeroway_cells");
    std::fs::create_dir_all(&folder).unwrap();
    let runway = cell_format::CellFeature {
        way_id: 42,
        class: cell_format::AERO_RUNWAY,
        is_polygon: false,
        name: Some("measured strip".into()),
        elevations: None,
        points: [(0., 0.), (0.04, 0.), (0.04, 0.002), (0., 0.002), (0., 0.)]
            .into_iter()
            .map(|(lon, lat)| cell_format::CellPoint { lon, lat })
            .collect(),
    };
    std::fs::write(
        folder.join(cell_format::cell_filename("aeroway", 0, 0)),
        cell_format::write::write_cell(0, 0, &[(cell_format::TAG_AERO, &[runway])]),
    )
    .unwrap();
    let loaded = load_features_from_cells(
        dir.path(),
        "aeroway",
        osm_ingest::GeoBounds {
            min_lat: 0.,
            max_lat: 0.1,
            min_lon: 0.,
            max_lon: 0.1,
        },
    );
    assert_eq!(loaded.len(), 1);
    assert!(!loaded[0].is_polygon);
    let prepared = InfraFeature::prepare(loaded.into_iter().next().unwrap(), "aeroway");
    assert!(
        shapes(&prepared)
            .iter()
            .any(|s| matches!(s, egui::Shape::Mesh(_)))
    );
    assert_eq!(prepared.way_id, 42);
}

#[test]
fn infrastructure_does_not_connect_across_missing_projections_or_fill_open_areas() {
    let line = InfraFeature::prepare(
        feature(
            "runway",
            false,
            &[(0., 0.), (1., 0.), (2., 0.), (3., 0.), (4., 0.)],
        ),
        "aeroway",
    );
    let split = line.shapes(
        egui::Rect::EVERYTHING,
        aeroway_stroke("runway"),
        aeroway_fill("runway"),
        |p, _| (p.lon != 2.).then_some(egui::pos2(p.lon, p.lat)),
    );
    assert_eq!(split.len(), 2);
    for part in split {
        let egui::Shape::Path(path) = part else {
            panic!("open strip")
        };
        assert_eq!(path.points.len(), 2);
        assert_eq!(path.points[1].x - path.points[0].x, 1.);
    }
    let open = InfraFeature::prepare(
        feature("terminal", true, &[(0., 0.), (1., 0.), (1., 1.)]),
        "aeroway",
    );
    assert!(open.fill.is_none());
}

#[test]
fn date_line_airport_footprints_keep_their_short_source_edges() {
    let strip = InfraFeature::prepare(
        feature(
            "runway",
            false,
            &[
                (179.9, 0.),
                (-179.9, 0.),
                (-179.9, 0.02),
                (179.9, 0.02),
                (179.9, 0.),
            ],
        ),
        "aeroway",
    );
    assert!(strip.fill.is_some());
    assert!(
        strip
            .points
            .windows(2)
            .all(|p| (p[1].0.lon - p[0].0.lon).abs() < 1.)
    );
}

fn nashville_features() -> Vec<InfraFeature> {
    let path =
        Path::new("/run/media/m/Minkowski/Derived/osm/aeroway_cells/aeroway_cell_+036_-0087.1kc");
    let bytes = std::fs::read(path).unwrap();
    cell_format::read::read_single_chunk(&bytes, cell_format::TAG_AERO)
        .unwrap()
        .into_iter()
        .filter(|f| {
            let west = f.points.iter().map(|p| p.lon).fold(f32::INFINITY, f32::min);
            let east = f
                .points
                .iter()
                .map(|p| p.lon)
                .fold(f32::NEG_INFINITY, f32::max);
            let south = f.points.iter().map(|p| p.lat).fold(f32::INFINITY, f32::min);
            let north = f
                .points
                .iter()
                .map(|p| p.lat)
                .fold(f32::NEG_INFINITY, f32::max);
            west <= -86.65 && east >= -86.72 && south <= 36.16 && north >= 36.09
        })
        .map(|f| {
            InfraFeature::prepare(
                LoadedPolyline {
                    way_id: f.way_id,
                    class: cell_format::decode_aero_class(f.class).into(),
                    name: f.name,
                    points: f
                        .points
                        .into_iter()
                        .map(|p| GeoPoint {
                            lat: p.lat,
                            lon: p.lon,
                        })
                        .collect(),
                    elevations: f.elevations,
                    is_polygon: f.is_polygon,
                },
                "aeroway",
            )
        })
        .collect()
}

#[test]
#[ignore = "reads existing Nashville aeroway cell on Minkowski"]
fn airports_live_cached_geometry_keeps_boundaries_and_runways_distinct() {
    let features = nashville_features();
    assert!(features.iter().any(|f| f.class == "runway"));
    let boundaries: Vec<_> = features
        .iter()
        .filter(|f| {
            matches!(
                f.class.as_str(),
                "dom_airport" | "intl_airport" | "airfield"
            )
        })
        .collect();
    assert!(!boundaries.is_empty());
    assert!(boundaries.iter().all(|f| f.fill.is_none()));
    for f in &features {
        eprintln!(
            "{} {:?}: {} source vertices, filled={}",
            f.class,
            f.name,
            f.points.len(),
            f.fill.is_some()
        );
    }
}

#[path = "admin_test_screen.rs"]
mod screen;

#[test]
#[ignore = "renders existing Nashville airport source geometry on a GPU"]
fn airports_live_gpu_preview_renders_runway_strips_without_boundary_fill() {
    let features = nashville_features();
    let mut screen = screen::Screen::new([1024, 768]);
    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 768.));
    let layout = super::super::local_terrain_scene::layout(rect);
    let focus = GeoPoint {
        lat: 36.125,
        lon: -86.685,
    };
    let mut view = GlobeViewState::from_focus(focus);
    view.local_mode = true;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        },
        |ctx| {
            let painter = ctx
                .layer_painter(egui::LayerId::background())
                .with_clip_rect(rect);
            painter.rect_filled(rect, 0., theme::canvas_background());
            for f in &features {
                painter.extend(f.shapes(
                    rect,
                    aeroway_stroke(&f.class),
                    aeroway_fill(&f.class),
                    |p, h| project_local(&layout, &view, focus, p, h, 3.5, 4.).map(|p| p.pos),
                ));
            }
        },
    );
    let rgba = screen.render(&ctx, output, true).unwrap();
    image::save_buffer(
        "/tmp/1kee-airports-preview.png",
        &rgba,
        1024,
        768,
        image::ColorType::Rgba8,
    )
    .unwrap();
    assert!(rgba.chunks_exact(4).any(|p| p[2] > 100));
}
