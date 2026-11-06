//! Native Moon/Mars batches, body switches, and optional read-only terrain QA.
use super::*;
use crate::panels::world_map::{
    contour_asset::{LocalTileGeometry, residency::Bounds},
    local_contour_pass::{LocalTileBatch, LocalTileId, occlusion_gpu_tests::Harness},
};
use std::time::{Duration, Instant};

fn wait_batches(
    model: &AppModel,
    tiles: &[LocalTileGeometry],
    ctx: &egui::Context,
) -> Vec<LocalTileBatch> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let painter = ctx.layer_painter(egui::LayerId::background());
    loop {
        let ready = batches(model, tiles, &painter);
        if ready.len() == tiles.len() {
            return ready;
        }
        assert!(
            Instant::now() < deadline,
            "native body batch build did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn draw_ready(
    h: &mut Harness,
    batches: &[LocalTileBatch],
    params: &projection::LocalProjectionParams,
    on: bool,
    size: [u32; 2],
) -> Vec<u8> {
    let rect =
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        h.render(batches, params, on, rect, &[]);
        if local_contour_pass::batches_uploaded(batches) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native body uploads did not complete"
        );
    }
    h.pixels()
}

fn parameters() -> projection::LocalProjectionParams {
    projection::LocalProjectionParams {
        focus_lon: 0.0,
        focus_lat: 0.0,
        x_factor: 1.0,
        y_factor: 1.0,
        z_factor: 0.0001,
        yaw_cos: 1.0,
        yaw_sin: 0.0,
        pitch_cos: 1.0,
        pitch_sin: 0.0,
        focus_center_x: 128.0,
        focus_center_y: 128.0,
        horizontal_scale: 100.0,
        ground_pitch_scale: 100.0,
        ground_depth_scale: 0.0,
        elevation_pitch_scale: 0.0,
        elevation_depth_scale: 60.0,
    }
}

#[test]
#[ignore = "requires GPU; optionally reads lunar/Mars caches from ONEKEE_CELESTIAL_CACHE_ROOT"]
fn celestial_local_gpu_preserves_classification_geometry_and_body_switches() {
    let mut h = Harness::new([256, 256]);
    h.palette = Some([
        egui::Color32::from_rgb(245, 125, 78),
        egui::Color32::from_rgb(96, 164, 181),
    ]);
    let mut model = AppModel::new();
    model.show_contours = true;
    let paths = Arc::new(
        [500.0, 1000.0, -500.0, -1000.0]
            .into_iter()
            .enumerate()
            .map(|(index, elevation_m)| {
                let lat = 0.6 - index as f32 * 0.4;
                contour_asset::ContourPath {
                    elevation_m,
                    points: vec![GeoPoint { lat, lon: -0.6 }, GeoPoint { lat, lon: 0.6 }],
                }
            })
            .collect::<Vec<_>>(),
    );
    let tile = LocalTileGeometry {
        id: LocalTileId {
            zoom_bucket: 99,
            lat_bucket: 0,
            lon_bucket: 0,
        },
        contours: paths.clone(),
        bounds: Bounds::from_contours(&paths),
    };
    let params = parameters();
    let mut previous = None;
    for body in [
        ActiveBody::Earth,
        ActiveBody::Moon,
        ActiveBody::Mars,
        ActiveBody::Earth,
    ] {
        model.active_body = body;
        let ready = wait_batches(&model, std::slice::from_ref(&tile), &h.ctx);
        assert_eq!(
            ready[0].instances.len(),
            4,
            "all body paths must reach the GPU"
        );
        assert_ne!(
            previous,
            Some(ready[0].version),
            "body switch reused another body's style"
        );
        previous = Some(ready[0].version);
        let pixels = draw_ready(&mut h, &ready, &params, false, [256, 256]);
        for path in paths.iter() {
            let (x, y, _) = params.project_logical(
                GeoPoint {
                    lon: 0.0,
                    lat: path.points[0].lat,
                },
                path.elevation_m,
            );
            let pixel = (-1..=1)
                .flat_map(|dy| {
                    (-1..=1).map(move |dx| {
                        ((y as i32 + dy) as usize * 256 + (x as i32 + dx) as usize) * 4
                    })
                })
                .map(|i| &pixels[i..i + 4])
                .max_by_key(|p| u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2]))
                .unwrap();
            let major = body == ActiveBody::Earth || path.elevation_m.abs() == 1000.0;
            assert!(
                pixel[0].max(pixel[2]) > 30,
                "missing {body:?} contour at {}m",
                path.elevation_m
            );
            assert_eq!(
                pixel[0] > pixel[2],
                major,
                "wrong {body:?} major/minor color at {}m: {pixel:?}",
                path.elevation_m
            );
        }
    }
    if let Ok(root) = std::env::var("ONEKEE_CELESTIAL_CACHE_ROOT") {
        for (body, file, label, colors) in [
            (
                ActiveBody::Moon,
                "lunar_focus_cache.sqlite",
                "moon",
                [
                    egui::Color32::from_rgb(215, 210, 188),
                    egui::Color32::from_rgb(138, 136, 126).gamma_multiply(0.78),
                ],
            ),
            (
                ActiveBody::Mars,
                "mars_ctx_cache.sqlite",
                "mars",
                [
                    egui::Color32::from_rgb(255, 145, 80),
                    egui::Color32::from_rgb(165, 85, 55).gamma_multiply(0.78),
                ],
            ),
        ] {
            cached_terrain(body, &std::path::Path::new(&root).join(file), label, colors);
        }
    }
}

fn cached_terrain(
    body: ActiveBody,
    path: &std::path::Path,
    label: &str,
    colors: [egui::Color32; 2],
) {
    let mut uri = reqwest::Url::from_file_path(path).unwrap();
    uri.set_query(Some("mode=ro&immutable=1"));
    let conn = rusqlite::Connection::open_with_flags(
        uri.as_str(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    let (zoom,lat,lon): (i32,i32,i32) = conn.query_row("SELECT zoom_bucket,lat_bucket,lon_bucket FROM contour_tile_manifest WHERE contour_count>5 LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).expect("nonempty cached celestial tile");
    let mut stmt = conn.prepare("SELECT lat_bucket,lon_bucket,elevation_m,geom FROM contour_tiles WHERE zoom_bucket=?1 AND lat_bucket BETWEEN ?2 AND ?3 AND lon_bucket BETWEEN ?4 AND ?5 ORDER BY lat_bucket,lon_bucket,fid").unwrap();
    let mut rows = stmt
        .query(rusqlite::params![zoom, lat - 1, lat + 1, lon - 1, lon + 1])
        .unwrap();
    let mut sources =
        std::collections::BTreeMap::<(i32, i32), Vec<contour_asset::ContourPath>>::new();
    while let Some(row) = rows.next().unwrap() {
        let elevation_m = row.get(2).unwrap();
        let bytes: Vec<u8> = row.get(3).unwrap();
        for points in tile_archive::gpkg::parse_gpkg_lines(&bytes, |lon, lat| GeoPoint { lon, lat })
        {
            sources
                .entry((row.get(0).unwrap(), row.get(1).unwrap()))
                .or_default()
                .push(contour_asset::ContourPath {
                    elevation_m,
                    points,
                });
        }
    }
    let composed = super::super::composition::compose(
        body,
        sources
            .into_iter()
            .map(|((lat_bucket, lon_bucket), paths)| {
                let contours = Arc::new(paths);
                super::super::composition::Source {
                    priority: 1,
                    tile: LocalTileGeometry {
                        id: LocalTileId {
                            zoom_bucket: zoom,
                            lat_bucket,
                            lon_bucket,
                        },
                        bounds: Bounds::from_contours(&contours),
                        contours,
                    },
                }
            })
            .collect(),
        Default::default(),
    )
    .frame;
    let paths = composed.contours.as_ref().unwrap();
    let bounds =
        Bounds::from_contours(paths).expect("cached celestial geometry survives core ownership");
    let [min, max] = [bounds.min, bounds.max];
    let mut params = parameters();
    params.focus_lon = (min[0] + max[0]) * 0.5;
    params.focus_lat = (min[1] + max[1]) * 0.5;
    params.x_factor = 2.0 / (max[0] - min[0]).max(0.00001);
    params.y_factor = 2.0 / (max[1] - min[1]).max(0.00001);
    params.z_factor = 1.0 / (max[2] - min[2]).max(1.0);
    params.horizontal_scale = 340.0;
    params.ground_pitch_scale = 110.0;
    params.elevation_depth_scale = 180.0;
    params.focus_center_x = 384.0;
    params.focus_center_y = 360.0 + min[2] * params.z_factor * params.elevation_depth_scale;
    let mut model = AppModel::new();
    model.active_body = body;
    model.show_contours = true;
    let mut h = Harness::new([768, 512]);
    h.palette = Some(colors);
    let ready = wait_batches(&model, &composed.tiles, &h.ctx);
    let expected: usize = paths.iter().map(|p| p.points.len().saturating_sub(1)).sum();
    assert_eq!(
        ready.iter().map(|b| b.instances.len()).sum::<usize>(),
        expected,
        "native body geometry was truncated"
    );
    for on in [false, true] {
        let pixels = draw_ready(&mut h, &ready, &params, on, [768, 512]);
        let lit = pixels
            .chunks_exact(4)
            .filter(|p| p[..3].iter().any(|c| *c > 10))
            .count();
        assert!(lit > 100, "{body:?} terrain invisible with occlusion={on}");
        println!(
            "{body:?}: {} owned tiles, {expected} native segments, {lit} lit pixels, occlusion={on}",
            ready.len()
        );
        image::save_buffer(
            format!("/tmp/1kee-celestial-{label}-{on}.png"),
            &pixels,
            768,
            512,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}
