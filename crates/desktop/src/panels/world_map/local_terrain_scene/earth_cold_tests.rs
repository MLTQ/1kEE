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
            center,
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
