use super::*;

#[path = "../admin_test_screen.rs"]
mod screen;

#[test]
fn fine_windows_respect_source_and_device_limits_and_join_at_the_dateline() {
    let view = [179.6, -0.3, 180.4, 0.3];
    let window = window::Window::plan(Product::TrueColor, view, 1024., 2048).unwrap();
    assert_eq!(window.columns, 163840);
    assert!(window.contains(view));
    assert!(window.contains([-180.4, -0.3, -179.6, 0.3]));
    let pieces: Vec<_> = window.segments().collect();
    assert_eq!(pieces.len(), 2);
    assert_eq!(pieces[0].1[2], 180.);
    assert_eq!(pieces[1].1[0], -180.);
    assert_eq!(pieces[1].0, pieces[0].2[0] as i32);
    assert_eq!(pieces[0].2[0] + pieces[1].2[0], window.size()[0]);
    assert_eq!(
        window::Window::plan(Product::Infrared, view, 1024., 2048)
            .unwrap()
            .columns,
        16384
    );
    let small = window::Window::plan(Product::TrueColor, view, 1024., 256).unwrap();
    assert!(small.size().into_iter().all(|d| d <= 256));
    assert!(small.columns < window.columns);
    assert!(
        window::Window::plan(Product::TrueColor, [-180., -90., 180., 90.], 1024., 2048).is_none()
    );
    let polar =
        window::Window::plan(Product::TrueColor, [22., 89.8, 23., 93.], 1024., 2048).unwrap();
    assert_eq!(polar.bounds()[3], 90.);
}

#[test]
fn detail_cache_is_bounded_and_keeps_entire_world_frames() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("weather.sqlite");
    cache::write(
        &path,
        Product::Infrared,
        "2026-11-08T15:00:00Z",
        b"overview",
    )
    .unwrap();
    for n in 0..70 {
        cache::store_detail(&path, &n.to_string(), &[n]).unwrap();
    }
    assert!(cache::detail_png(&path, "0").is_none());
    assert_eq!(cache::detail_png(&path, "69"), Some(vec![69]));
    assert_eq!(
        cache::read(&path, Product::Infrared).unwrap().1,
        b"overview"
    );
}

#[test]
fn cached_dateline_segments_preserve_color_alpha_and_pixel_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("weather.sqlite");
    let product = Product::Infrared;
    let stamp = "2026-11-08T15:00:00Z";
    let w = window::Window::plan(product, [179.5, 0.1, 180.5, 0.4], 1024., 2048).unwrap();
    let parts: Vec<_> = w.segments().collect();
    assert_eq!(parts.len(), 2);
    for &(offset, bounds, [width, height]) in &parts {
        let color = if offset == 0 {
            [0, 0, 0, 255]
        } else {
            [0, 0, 0, 0]
        };
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let key = format!("{}|{stamp}|{bounds:?}|{:?}", product.id(), [width, height]);
        cache::store_detail(&path, &key, png.get_ref()).unwrap();
    }
    let loaded = detail::fetch(product, stamp, w, &path).unwrap();
    assert_eq!(loaded.bounds, w.bounds());
    assert_eq!(loaded.pixels.size, w.size().map(|n| n as usize));
    let seam = parts[1].0 as usize;
    for row in loaded.pixels.pixels.chunks_exact(loaded.pixels.size[0]) {
        assert_eq!(row[seam - 1], egui::Color32::BLACK);
        assert_eq!(row[seam], egui::Color32::TRANSPARENT);
    }
}

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
            let output = paint(&ctx, &gpu, rect, &model);
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
    // The same stamped product must change from the world overview to a
    // genuinely sharper provider response through both production draw paths.
    for product in 0..2 {
        model.global_weather.feeds[0].options.enabled = product == 0;
        model.global_weather.feeds[1].options.enabled = product == 1;
        model.globe_view.focus_on(if product == 0 {
            crate::model::GeoPoint {
                lat: 24.4,
                lon: 54.35,
            }
        } else {
            crate::model::GeoPoint {
                lat: 38.,
                lon: -38.,
            }
        });
        for local in [false, true] {
            *model.global_weather.feeds[product].detail.get_mut() = detail::Detail::default();
            model.globe_view.local_mode = local;
            model.globe_view.zoom = 45.;
            model.globe_view.local_zoom = if product == 0 { 7.5 } else { 4.5 };
            model.globe_view.local_yaw = 0.;
            model.globe_view.local_pitch = 0.35;
            let first_stamp = model.global_weather.feeds[product].stamp.clone();
            let deadline = Instant::now() + Duration::from_secs(120);
            let mut overview = None;
            loop {
                let output = paint(&ctx, &gpu, rect, &model);
                let detail = model.global_weather.feeds[product].detail.borrow();
                assert!(!detail.status.contains("unavailable"), "{}", detail.status);
                let ready = detail.status.starts_with("Detail ·");
                let pixels = gpu.render(&ctx, output, overview.is_none() || ready);
                if ready {
                    let before = overview.unwrap();
                    let pixels = pixels.unwrap();
                    assert!(pixels != before, "no change at the same camera and time");
                    assert_eq!(first_stamp, model.global_weather.feeds[product].stamp);
                    println!(
                        "{} in {}: {}",
                        model.global_weather.feeds[product].product.label(),
                        if local { "Local" } else { "Globe" },
                        detail.status
                    );
                    if let Some(dir) = std::env::var_os("ONEKEE_WEATHER_PREVIEWS") {
                        let mode = if local { "local" } else { "globe" };
                        for (quality, image) in [("overview", before), ("detail", pixels)] {
                            image::save_buffer(
                                PathBuf::from(&dir)
                                    .join(format!("weather-{product}-{mode}-{quality}.png")),
                                &image,
                                1024,
                                640,
                                image::ColorType::Rgba8,
                            )
                            .unwrap();
                        }
                    }
                    break;
                } else if overview.is_none() {
                    overview = pixels;
                }
                assert!(Instant::now() < deadline, "timed out: {}", detail.status);
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
}

fn paint(
    ctx: &egui::Context,
    gpu: &screen::Screen,
    rect: egui::Rect,
    model: &AppModel,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(rect),
            max_texture_side: Some(gpu.max_texture_side()),
            ..Default::default()
        },
        |ctx| {
            let painter = ctx
                .layer_painter(egui::LayerId::background())
                .with_clip_rect(rect);
            if model.globe_view.local_mode {
                super::super::local_terrain_scene::paint(&painter, rect, model, 0.0);
            } else {
                super::super::globe_scene::paint(&painter, rect, model, 0.0);
            }
            model.global_weather.draw_time(&painter, rect);
        },
    )
}

#[test]
#[ignore = "plays published historical imagery from both providers through the GPU renderer"]
fn live_satellite_playback() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut gpu = screen::Screen::new([960, 600]);
    gpu.install_globe();
    let mut model = AppModel::new();
    model.map_theme = crate::theme::MapTheme::Topo;
    model.obs_view = true;
    model.selected_root = None;
    model.show_contours = false;
    model.show_bathymetry = false;
    model.show_beam = false;
    model.globe_view.focus_on(crate::model::GeoPoint {
        lat: 18.,
        lon: -49.,
    });
    model.globe_view.zoom = 1.;
    model.globe_view.local_zoom = 1.;
    model.globe_view.local_pitch = 0.35;
    let path = dir.path().join("weather.sqlite");
    model.global_weather = GlobalWeather::new(Default::default(), path.clone());
    for feed in &mut model.global_weather.feeds {
        feed.options.enabled = true;
    }
    let deadline = Instant::now() + Duration::from_secs(300);
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
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(30));
    }
    for feed in &model.global_weather.feeds {
        assert!(
            feed.catalog.len() >= 8 && feed.image.is_some(),
            "{}: {}",
            feed.product.label(),
            feed.status
        );
    }
    model.global_weather.player = Some(playback::Player::new(&model.global_weather.feeds));
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(960., 600.));
    let mut times = Vec::new();
    let mut previous: [Option<Vec<u8>>; 2] = Default::default();
    let mut changes = [0; 2];
    while times.len() < 9 {
        model.global_weather.tick(&ctx, ActiveBody::Earth);
        let player = model.global_weather.player.as_ref().unwrap();
        assert!(
            player.playing,
            "playback stopped on an unavailable observation"
        );
        if let Some(time) = player.shown_time()
            && times.last().is_none_or(|old| old != time)
        {
            assert!(times.last().is_none_or(|old: &String| old.as_str() < time));
            assert_eq!(model.global_weather.displayed(0).unwrap().0, &time[..10]);
            assert_eq!(model.global_weather.displayed(1).unwrap().0, time);
            for (i, prior) in previous.iter_mut().enumerate() {
                model.globe_view.local_mode = i == 1;
                let out = paint(&ctx, &gpu, rect, &model);
                let pixels = gpu.render(&ctx, out, true).unwrap();
                // Exclude the clock and compare only the map.
                let scene = &pixels[960 * 4 * 60..];
                if prior.as_ref().is_some_and(|old| scene != old.as_slice()) {
                    changes[i] += 1;
                }
                *prior = Some(scene.to_vec());
                if i == 0
                    && let Some(dir) = std::env::var_os("ONEKEE_WEATHER_PREVIEWS")
                {
                    image::save_buffer(
                        PathBuf::from(dir).join(format!("playback-{:02}.png", times.len())),
                        &pixels,
                        960,
                        600,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
            println!("Rendered globe and local at {time}");
            times.push(time.to_owned());
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {} frames",
            times.len()
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_ne!(&times[0][..10], &times[8][..10]);
    // A regional crop can legitimately repeat between two three-hour mosaics.
    assert!(
        changes.iter().all(|&n| n >= 2),
        "changes per view: {changes:?}"
    );
    model.global_weather.player.as_mut().unwrap().playing = false;
    model.globe_view.local_zoom = 4.5;
    loop {
        let out = paint(&ctx, &gpu, rect, &model);
        gpu.render(&ctx, out, false);
        let ir = &model.global_weather.feeds[1];
        let detail = ir.detail.borrow();
        if detail.status.starts_with("Detail ·") {
            break;
        }
        assert!(
            !detail.status.contains("unavailable") && Instant::now() < deadline,
            "{}",
            detail.status
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        model.global_weather.displayed(1).unwrap().0,
        times.last().unwrap()
    );
    model.global_weather.stop_playback();
    for (i, feed) in model.global_weather.feeds.iter().enumerate() {
        assert_eq!(
            model.global_weather.displayed(i).unwrap().0,
            feed.stamp.as_deref().unwrap()
        );
    }
}
