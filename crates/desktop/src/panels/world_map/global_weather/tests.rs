use super::*;

#[path = "../admin_test_screen.rs"]
mod screen;

#[test]
fn both_layers_are_opt_in_and_a_failed_check_keeps_the_displayed_frame() {
    let dir = tempfile::tempdir().unwrap();
    let mut weather = GlobalWeather::new(Default::default(), dir.path().join("cache.sqlite"));
    let ctx = egui::Context::default();
    weather.tick(&ctx, ActiveBody::Earth);
    assert!(weather.feeds.iter().all(|feed| feed.worker.is_none()));
    weather.feeds[0].options.enabled = true;
    weather.tick(&ctx, ActiveBody::Moon);
    assert!(weather.feeds[0].worker.is_none());

    let (tx, rx) = mpsc::channel();
    weather.feeds[0].worker = Some(rx);
    let stamp = "2026-11-08";
    tx.send(Message::Frame {
        stamp: stamp.into(),
        image: SatelliteImage {
            bounds: [-180., -90., 180., 90.],
            pixels: egui::ColorImage::new([2, 1], egui::Color32::WHITE),
        },
        cache_warning: None,
    })
    .unwrap();
    tx.send(Message::Done(Err("offline".into()))).unwrap();
    weather.tick(&ctx, ActiveBody::Earth);
    assert_eq!(weather.images().count(), 1);
    assert_eq!(weather.feeds[0].stamp.as_deref(), Some(stamp));
    assert!(weather.feeds[0].status.contains("offline"));
    weather.feeds[0].options.enabled = false;
    assert_eq!(weather.images().count(), 0);
}

#[test]
#[ignore = "downloads both published global frames and requires a graphics adapter"]
fn live_global_weather_renders_both_products_on_globe_and_local_views() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut gpu = screen::Screen::new([1024, 640]);
    gpu.install_globe();
    let mut model = AppModel::new();
    model.map_theme = crate::theme::MapTheme::Topo;
    model.obs_view = true;
    model.selected_root = None;
    model.show_contours = false;
    model.show_bathymetry = false;
    model.show_beam = false;
    model.globe_view.focus_on(crate::model::GeoPoint {
        lat: 12.0,
        lon: -20.0,
    });
    model.globe_view.zoom = 0.8;
    let path = dir.path().join("weather.sqlite");
    model.global_weather = GlobalWeather::new(Settings::default(), path.clone());
    for feed in &mut model.global_weather.feeds {
        feed.options.enabled = true;
    }
    let limit = Instant::now() + Duration::from_secs(200);
    loop {
        model.global_weather.tick(&ctx, ActiveBody::Earth);
        if model
            .global_weather
            .feeds
            .iter()
            .all(|f| f.worker.is_none())
        {
            break;
        }
        assert!(Instant::now() < limit, "weather sources timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    for feed in &model.global_weather.feeds {
        assert!(
            feed.status.is_empty() && feed.image.is_some(),
            "{}: {}",
            feed.product.label(),
            feed.status
        );
        assert_eq!(
            cache::read(&path, feed.product).unwrap().0,
            *feed.stamp.as_ref().unwrap()
        );
        println!(
            "{}: {}",
            feed.product.label(),
            feed.stamp.as_deref().unwrap()
        );
    }
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 640.));
    for local in [false, true] {
        model.globe_view.local_mode = local;
        model.globe_view.local_zoom = 1.0;
        let mut previous = None;
        for (color, infrared) in [(false, false), (true, false), (true, true), (false, true)] {
            model.global_weather.feeds[0].options.enabled = color;
            model.global_weather.feeds[1].options.enabled = infrared;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    max_texture_side: Some(gpu.max_texture_side()),
                    ..Default::default()
                },
                |ctx| {
                    let painter = ctx
                        .layer_painter(egui::LayerId::background())
                        .with_clip_rect(rect);
                    if local {
                        super::super::local_terrain_scene::paint(&painter, rect, &model, 0.0);
                    } else {
                        super::super::globe_scene::paint(&painter, rect, &model, 0.0);
                    }
                },
            );
            let pixels = gpu.render(&ctx, output, true).unwrap();
            if let Some(old) = previous.replace(pixels.clone()) {
                assert!(
                    pixels != old,
                    "toggling one layer must change the framebuffer"
                );
            }
            if !local
                && color
                && let Some(dir) = std::env::var_os("ONEKEE_WEATHER_PREVIEWS")
            {
                let name = if infrared {
                    "global-weather-combined.png"
                } else {
                    "global-weather-true-color.png"
                };
                image::save_buffer(
                    PathBuf::from(dir).join(name),
                    &pixels,
                    1024,
                    640,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }
}
