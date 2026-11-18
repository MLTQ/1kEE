//! Fresh-destination Local benchmark. Run one engine per process with isolated settings.
use super::*;
use crate::panels::world_map::local_contour_pass::occlusion_gpu_tests::Harness;
use one_thousand_electric_eye_cache_builder::gpu_terrain;
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires an isolated settings directory, ONEKEE_SRTM_ROOT, GDAL and a GPU"]
fn cold_earth_local_nine_tiles() {
    let root = tempfile::tempdir().unwrap();
    let selected = Some(root.path());
    let derived = root.path().join("Derived");
    let source = std::fs::canonicalize(std::env::var_os("ONEKEE_SRTM_ROOT").unwrap()).unwrap();
    std::fs::create_dir(&derived).unwrap();
    std::os::unix::fs::symlink(&source, root.path().join("SRTM_GL1_srtm")).unwrap();
    assert_eq!(
        crate::terrain_assets::find_derived_root(selected),
        Some(derived)
    );
    assert_eq!(
        std::fs::canonicalize(crate::terrain_assets::find_srtm_root(selected).unwrap()).unwrap(),
        source
    );
    assert!(
        srtm_focus_cache::recovery::isolated_root().is_none(),
        "use isolated test settings"
    );

    let mut screen = Harness::new([384, 384]);
    gpu_terrain::register_desktop_adapter(screen.adapter_info.clone());
    let mut model = AppModel::new();
    model.active_body = ActiveBody::Earth;
    model.show_contours = true;
    model.contour_occlusion = false;
    let (zoom, lat, lon, extent_km) = if std::env::var_os("ONEKEE_LOCAL_WIDE").is_some() {
        (0.5, 10., 28., 340.)
    } else {
        (5., 126., 143., 52.)
    };
    let step = srtm_focus_cache::half_extent_for_zoom(zoom) * 0.45;
    let center = GeoPoint {
        lat: lat * step,
        lon: lon * step,
    };
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(384., 384.));
    let layout = layout(rect);
    let params = projection::local_projection_params(
        &layout,
        &model.globe_view,
        center,
        extent_km,
        extent_km,
    );
    contour_asset::set_local_viewport(ActiveBody::Earth, params, rect);
    let painter = screen.ctx.layer_painter(egui::LayerId::background());
    let start = Instant::now();
    let mut next = 0;
    let milestones = [1, 5, 9];
    loop {
        let load = contour_asset::load_srtm_lod_region(
            selected,
            None,
            center,
            zoom,
            zoom,
            1,
            screen.ctx.clone(),
        );
        let ready = batches(&model, &load.tiles, &painter);
        screen.render(&ready, &params, false, rect, &[]);
        let count = ready.len() - local_contour_pass::batches_pending_upload(&ready);
        while next < milestones.len() && count >= milestones[next] {
            eprintln!(
                "COLD_LOCAL gpu={} tiles={} elapsed_ms={:.1}",
                gpu_terrain::enabled(),
                milestones[next],
                start.elapsed().as_secs_f64() * 1000.
            );
            next += 1;
        }
        if next == milestones.len() {
            assert!(ready.iter().all(|b| !b.instances.is_empty()));
            assert!(screen.pixels().chunks_exact(4).any(|px| px[0] > 30));
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(180),
            "only {count} tiles rendered"
        );
        std::thread::sleep(Duration::from_millis(16));
    }
    srtm_focus_cache::shutdown_terrain_jobs();
}

/// Time the center and its eight neighboring tiles through a real Follow Events
/// flight and the Local renderer's composition, instance and GPU publication.
#[test]
#[ignore = "requires isolated settings, ONEKEE_SRTM_ROOT and the RTX 4090"]
fn cold_follow_event_arrival() {
    let step = lod::step(ActiveBody::Earth, 5.0);
    measure_follow_arrival(GeoPoint {
        lat: 126. * step,
        lon: 143. * step,
    });
}

#[test]
#[ignore = "requires isolated settings, ONEKEE_SRTM_ROOT and the RTX 4090"]
fn cold_follow_tiraspol_arrival() {
    measure_follow_arrival(GeoPoint {
        lat: 46.84275,
        lon: 29.6284,
    });
}

fn measure_follow_arrival(stop: GeoPoint) {
    use crate::model::{EventRecord, EventSeverity, FactalBrief};
    use local_contour_pass::LocalBatchId;
    use std::collections::HashSet;

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("Derived")).unwrap();
    let source = std::fs::canonicalize(std::env::var_os("ONEKEE_SRTM_ROOT").unwrap()).unwrap();
    std::os::unix::fs::symlink(source, root.path().join("SRTM_GL1_srtm")).unwrap();
    let mut screen = Harness::new([960, 540]);
    gpu_terrain::register_desktop_adapter(screen.adapter_info.clone());
    assert!(gpu_terrain::enabled());
    if let Some(repairs) = srtm_focus_cache::recovery::isolated_root() {
        assert_eq!(
            std::env::var_os("ONEKEE_TERRAIN_REPAIR_ROOT").as_deref(),
            Some(repairs.as_os_str()),
            "use an explicit temporary repair root instead of saved desktop settings"
        );
        assert_eq!(
            repairs.read_dir().unwrap().count(),
            0,
            "expected an empty temporary repair root"
        );
    }

    let mut model = AppModel::new();
    model.selected_root = Some(root.path().to_owned());
    model.active_body = crate::model::ActiveBody::Earth;
    model.show_contours = true;
    model.contour_occlusion = false;
    model.globe_view.local_mode = true;
    model.globe_view.local_zoom = 9.5;
    model.globe_view.local_center = GeoPoint {
        lat: 39.93,
        lon: 32.86,
    };
    let step = lod::step(model.active_body, 5.0);
    let lat_bucket = (stop.lat / step).round() as i32;
    let lon_bucket = (stop.lon / step).round() as i32;
    model.event_follow.enable(&[EventRecord {
        id: "factal-benchmark".into(),
        title: "Benchmark".into(),
        summary: String::new(),
        severity: EventSeverity::Advisory,
        location_name: String::new(),
        location: stop,
        source: "fixture".into(),
        occurred_at: String::new(),
        factal_brief: Some(FactalBrief {
            factal_id: "benchmark".into(),
            severity_value: Some(1),
            occurred_at_raw: None,
            point_wkt: None,
            vertical: None,
            subvertical: None,
            topics: vec![],
            content: None,
            raw_json_pretty: String::new(),
        }),
    }]);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(960., 540.));
    let layout = layout(rect);
    let painter = screen.ctx.layer_painter(egui::LayerId::background());
    let start = Instant::now();
    let mut previous = None;
    let mut milestones = [false; 3];
    let mut arrival = None;
    let mut next_report = 10.0;
    loop {
        let now = start.elapsed().as_secs_f64();
        model.event_follow.tick(&mut model.globe_view, now);
        let view = &model.globe_view;
        let center = view.local_center;
        if arrival.is_none() && model.event_follow.event_label_opacity(now) > 0.0 {
            arrival = Some(now);
            eprintln!("FOLLOW_LOCAL arrival_s={now:.3}");
        }
        let zoom = local_render_zoom(view.local_zoom);
        let extent = visual_half_extent_for_zoom(zoom) * 111.32;
        let projection = projection::local_projection_params(
            &layout,
            view,
            center,
            (extent * center.lat.to_radians().cos().abs().max(0.2)).max(1.),
            extent.max(1.),
        );
        contour_asset::set_local_viewport(model.active_body, projection, rect);
        let viewport = contour_asset::residency::Viewport {
            body: model.active_body,
            projection,
            rect: rect.expand(8.),
        };
        let selection = lod::select(model.active_body, zoom, previous);
        previous = Some(selection);
        let load = earth_contours_for_view(&model, selection, now, &screen.ctx);
        let frame = handoff::prepare(model.selected_root.as_deref(), viewport, &load, &screen.ctx);
        let prepared = batches(&model, &frame.tiles, &painter);
        screen.render(&prepared, &projection, false, rect, &[]);
        let display = handoff::select(viewport, frame, prepared, true, &screen.ctx);
        let inner: HashSet<_> = display
            .batches
            .iter()
            .filter_map(|batch| match batch.id {
                LocalBatchId::Contour(id)
                    if id.zoom_bucket == 4
                        && (id.lat_bucket - lat_bucket).abs() <= 1
                        && (id.lon_bucket - lon_bucket).abs() <= 1
                        && !batch.instances.is_empty() =>
                {
                    Some((id.lat_bucket, id.lon_bucket))
                }
                _ => None,
            })
            .collect();
        for (i, (name, ready)) in [
            ("center", inner.contains(&(lat_bucket, lon_bucket))),
            ("five", inner.len() >= 5),
            ("nine", inner.len() == 9),
        ]
        .into_iter()
        .enumerate()
        {
            if ready && !milestones[i] {
                milestones[i] = true;
                screen.render(&display.batches, &projection, false, rect, &[]);
                assert!(screen.pixels().chunks_exact(4).any(|px| px[0] > 30));
                eprintln!("FOLLOW_LOCAL {name}_s={:.3}", start.elapsed().as_secs_f64());
            }
        }
        if now >= next_report {
            next_report += 10.0;
            eprintln!(
                "FOLLOW_LOCAL elapsed_s={now:.1} ready={}/{} pending={} decoded={} displayed={}",
                load.status.ready_assets,
                load.status.total_assets,
                load.status.pending_assets,
                load.tiles.len(),
                display.batches.len(),
            );
        }
        if milestones.iter().all(|m| *m)
            && arrival.is_some()
            && load.status.total_assets > 0
            && load.status.ready_assets == load.status.total_assets
        {
            let displayed: HashSet<_> = display.batches.iter().map(|b| b.id).collect();
            let visible: Vec<_> = load
                .tiles
                .iter()
                .filter(|t| {
                    !t.contours.is_empty()
                        && viewport.intersects(composition::visible_bounds(model.active_body, t))
                })
                .collect();
            if !visible.is_empty()
                && visible
                    .iter()
                    .all(|t| displayed.contains(&LocalBatchId::Contour(t.id)))
            {
                screen.render(&display.batches, &projection, false, rect, &[]);
                eprintln!(
                    "FOLLOW_LOCAL visible_s={:.3} visible_tiles={}",
                    start.elapsed().as_secs_f64(),
                    visible.len()
                );
                break;
            }
        }
        assert!(
            now < 90.,
            "event core milestones {milestones:?}, visible {inner:?}"
        );
        std::thread::sleep(Duration::from_millis(16));
    }
    srtm_focus_cache::shutdown_terrain_jobs();
}
