use super::*;

fn map_frame(
    ctx: &egui::Context,
    model: &mut AppModel,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(800., 600.),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(780., 580.), egui::Sense::click_and_drag());
                let pointer = response.hover_pos().and_then(|pos| {
                    PickRegion::from_screen(pos, |p| {
                        Some(GeoPoint {
                            lon: p.x / 100.,
                            lat: p.y / 100.,
                        })
                    })
                });
                map_interaction(ui, &response, model, pointer);
            });
        },
    );
    assert!(
        !output
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command, egui::OutputCommand::OpenUrl(_)))
    );
    output
}

fn click(ctx: &egui::Context, model: &mut AppModel, at: Pos2, button: egui::PointerButton) {
    for pressed in [true, false] {
        map_frame(
            ctx,
            model,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
        );
    }
}

fn wait_read(ctx: &egui::Context, model: &mut AppModel) {
    let limit = Instant::now() + Duration::from_secs(5);
    while model.lidar.map_pick.job.is_some() {
        assert!(Instant::now() < limit);
        std::thread::sleep(Duration::from_millis(2));
        map_frame(ctx, model, vec![]);
    }
}

#[test]
fn right_click_single_edge_and_choose_overlapping_acquisitions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(r#"CREATE TABLE surveys(id,source,name,product,date,page,asset,stac,geometry,west,south,east,north);
        INSERT INTO surveys VALUES('NOAAID:1','noaa','Coastal strip','Lidar','1998','','',NULL,
            '{"type":"Polygon","coordinates":[[[1,1],[3,1],[3,2],[1,2],[1,1]]]}',1,1,3,2);
        INSERT INTO surveys VALUES('G3012896672-ORNL_CLOUD:ST1.laz','nasa-brazil','Coastal strip','NASA lidar point cloud','2018','https://search.earthdata.nasa.gov/example','https://data.ornldaac.earthdata.nasa.gov/protected/example.laz',NULL,
            '{"type":"Polygon","coordinates":[[[2,1],[4,1],[4,2],[2,2],[2,1]]]}',2,1,4,2);"#).unwrap();
    let ctx = egui::Context::default();
    ctx.style_mut(|s| s.animation_time = 0.);
    let mut model = AppModel::new();
    model.lidar = Browser::new(path);
    model.lidar.set_enabled(true);
    model
        .lidar
        .tiles
        .draw(Some([-180., -90., 180., 90.]), |_| {});
    map_frame(&ctx, &mut model, vec![]);
    let right = egui::PointerButton::Secondary;
    // Three pixels above the edge is still a hit. An overview pick must land on the strip.
    click(&ctx, &mut model, egui::pos2(140., 97.), right);
    wait_read(&ctx, &mut model);
    assert_eq!(model.lidar.selected.as_ref().unwrap().id, "NOAAID:1");
    assert!(model.globe_view.local_mode);
    let focus = model.globe_view.local_center;
    assert!((focus.lon - 1.4).abs() < 1e-4 && (focus.lat - 1.).abs() < 1e-4);
    model.lidar.tiles.draw(Some([1., 1., 3., 3.]), |_| {});
    let zoom = model.globe_view.local_zoom;
    click(&ctx, &mut model, egui::pos2(250., 150.), right);
    wait_read(&ctx, &mut model);
    assert_eq!(model.lidar.selected.as_ref().unwrap().id, "NOAAID:1");
    let menu = map_frame(&ctx, &mut model, vec![]);
    let item = |name| {
        menu.shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == name => {
                    Some(t.pos + t.galley.size() / 2.)
                }
                _ => None,
            })
            .expect(name)
    };
    item("Choose a lidar survey");
    assert!(item("1998   Coastal strip").y < item("2018   Coastal strip").y);
    click(
        &ctx,
        &mut model,
        item("2018   Coastal strip"),
        egui::PointerButton::Primary,
    );
    assert_eq!(
        model.lidar.selected.as_ref().unwrap().id,
        "G3012896672-ORNL_CLOUD:ST1.laz"
    );
    assert_eq!(model.globe_view.local_center, focus);
    assert_eq!(model.globe_view.local_zoom, zoom);
    assert!(model.lidar.map_pick.click.is_none());
    model.lidar.set_enabled(false);
    model.lidar.set_enabled(true);
    assert!(model.lidar.selected.is_none() && !model.lidar.automatic);
}

#[test]
fn survey_holes_and_dateline_edges_use_a_screen_space_tolerance() {
    let area = GeoJsonLayer::parse(String::new(), r#"{"type":"Polygon","coordinates":[[[179,0],[-179,0],[-179,2],[179,2],[179,0]],[[179.7,0.5],[-179.7,0.5],[-179.7,1.5],[179.7,1.5],[179.7,0.5]]]}"#).unwrap();
    let geom = &area.features[0].geometry;
    let hit = |lon, lat| {
        PickRegion {
            at: GeoPoint { lon, lat },
            dx: Vec2::X * 0.05,
            dy: Vec2::Y * 0.05,
        }
        .hit(geom)
    };
    assert!(hit(-179.5, 1.).is_some());
    assert!(hit(179.9, 1.).is_none());
    assert!(hit(178.97, 1.).is_some());
    assert!(hit(178.90, 1.).is_none());
    assert!(hit(0., 1.).is_none());
}

#[test]
#[ignore = "requires the Ocean City surveys in ONEKEE_TEST_LIDAR_DB"]
fn map_pick_finds_winter_atm_among_ocean_city_surveys() {
    let path = std::env::var_os("ONEKEE_TEST_LIDAR_DB").unwrap();
    let at = GeoPoint {
        lon: -75.0883,
        lat: 38.33,
    };
    let hits = lookup(Path::new(&path), PickRegion::point(at), true).unwrap();
    assert!(hits.iter().filter(|h| h.available).count() > 1);
    assert!(
        hits.iter()
            .any(|h| h.available && h.survey.id == "NOAAID:38" && h.focus == at)
    );
}
