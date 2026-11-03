//! Optional GPU verification of actual admin controls and rendering.
use super::*;
use crate::theme::MapTheme;
use eframe::{egui_wgpu, wgpu};
fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    use std::{
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
    };
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park_timeout(std::time::Duration::from_millis(10)),
        }
    }
}
#[test]
#[ignore = "requires GPU; optional ONEKEE_ADMIN_PREVIEW PNG"]
fn admin_gpu_preview_preserves_holes_and_toggle_round_trip() {
    preview(false, false, MapTheme::Topo);
}

#[test]
#[ignore = "requires GPU; optional ONEKEE_ADMIN_GLOBE_PREVIEW PNG"]
fn admin_globe_gpu_preserves_holes_and_hides_backside() {
    for theme in [MapTheme::Sodium, MapTheme::Topo] {
        let globe = preview(true, false, theme);
        let local = preview(false, false, theme);
        for (a, b) in globe.into_iter().zip(local).take(3) {
            assert!(
                a.abs_diff(b) <= 2,
                "{theme:?} fill must match between Globe and Local: {globe:?} vs {local:?}"
            );
        }
        // The same state polygon is amber under Sodium and cyan under Topo.
        assert_eq!(local[0] > local[2], theme == MapTheme::Sodium);
    }
}

#[test]
#[ignore = "requires GPU and ONEKEE_ADMIN_REAL_CACHE with published Andorra geometry"]
fn admin_real_cache_renders_in_both_views() {
    preview(false, true, MapTheme::Topo);
    preview(true, true, MapTheme::Topo);
}

fn preview(globe: bool, real: bool, theme: MapTheme) -> [u8; 4] {
    let scratch = tests::Scratch::new();
    let dir = scratch.0.join("admin_cells");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(admin_filename(4)),
        cell_format::write::write_cell(
            0,
            0,
            &[(
                TAG_ADMN,
                &[
                    tests::square(10, 4, 0., 6.),
                    tests::square(10, 4, 2., 3.),
                    tests::square(11, 4, 7., 9.),
                ],
            )],
        ),
    )
    .unwrap();
    let root = if real {
        PathBuf::from(std::env::var("ONEKEE_ADMIN_REAL_CACHE").expect("published cache root"))
    } else {
        scratch.0.clone()
    };
    let level = if real { 2 } else { 4 };
    let mut data = load_from_dirs(std::slice::from_ref(&root), &[level]);
    if real {
        data.regions.retain(|r| r.relation_id == 9407);
        assert_eq!(data.regions.len(), 1, "published Andorra relation");
        assert!(
            data.regions[0].fill.is_some(),
            "real country has complete topology"
        );
        assert!(data.regions[0].rings.iter().map(Vec::len).sum::<usize>() > 100);
    }
    let data = Arc::new(data);
    let focus = if real {
        GeoPoint {
            lat: 42.55,
            lon: 1.58,
        }
    } else {
        GeoPoint { lat: 4.0, lon: 4.0 }
    };
    let local_project = |p: GeoPoint| {
        if real {
            egui::pos2(
                390. + (p.lon - focus.lon) * 1500.,
                400. - (p.lat - focus.lat) * 2000.,
            )
        } else {
            egui::pos2(70. + p.lon * 65., 640. - p.lat * 55.)
        }
    };
    let mut model = crate::model::AppModel::new();
    model.map_theme = theme;
    model.set_contour_stroke_width_px(2.0);
    model.selected_root = Some(root.clone());
    model.admin_options.cache_root = Some(root);
    model.show_admin = true;
    model.globe_view = crate::model::GlobeViewState::from_focus(focus);
    model.globe_view.local_mode = !globe;
    model.admin_options.enabled = [false; 10];
    model.admin_options.enabled[level as usize - 2] = true;
    model.admin_options.opacity = 0.3;
    let adapter = block_on(wgpu::Instance::default().request_adapter(&Default::default()))
        .expect("GPU adapter");
    let (device, queue) = block_on(adapter.request_device(&Default::default(), None)).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
    renderer
        .callback_resources
        .insert(super::super::contour_pass::ContourPassResources::new(
            &device, format,
        ));
    let layout = super::super::globe_scene::GlobeLayout {
        center: egui::pos2(390., 384.),
        radius: if real { 3300. } else { 330. },
        focal_length: 2.2,
        camera_distance: if real { 1.06 } else { 1.25 },
    };
    let batch =
        super::super::admin_globe::prepare(Arc::clone(&data), model.map_theme.admin_palette());
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 768.));
    let map = egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(780., 768.));
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [1024, 768],
        pixels_per_point: 1.,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("admin QA"),
        size: wgpu::Extent3d {
            width: 1024,
            height: 768,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let mut pictures = Vec::new();
    for (shade, back, opacity) in [
        (false, false, 1.0),
        (true, false, 1.0),
        (false, false, 1.0),
        (false, true, 1.0),
        (true, true, 1.0),
        (true, false, 0.5),
        (true, false, 0.0),
        (false, false, 0.0),
        (false, false, 0.5),
        (true, false, 1.0),
    ] {
        model.globe_view = crate::model::GlobeViewState::from_focus(if back {
            GeoPoint {
                lat: -focus.lat,
                lon: focus.lon - 180.0,
            }
        } else {
            focus
        });
        model.admin_options.shade = shade;
        model.admin_options.layer_opacity = opacity;
        for frame in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ctx| {
                    egui::SidePanel::right("admin-qa-controls")
                        .exact_width(230.)
                        .resizable(false)
                        .show(ctx, |ui| {
                            ui.checkbox(&mut model.show_admin, "Admin Boundaries");
                            super::super::admin_controls::controls(ui, &mut model);
                        });
                    let painter = ctx
                        .layer_painter(egui::LayerId::background())
                        .with_clip_rect(map);
                    painter.add(super::super::contour_pass::residency_callback(map));
                    painter.text(
                        egui::pos2(40., 60.),
                        egui::Align2::LEFT_TOP,
                        if real {
                            if globe {
                                "Andorra · real PBF boundaries · Globe"
                            } else {
                                "Andorra · real PBF boundaries · Local"
                            }
                        } else {
                            "Administrative boundaries · synthetic QA geometry"
                        },
                        egui::FontId::proportional(18.),
                        egui::Color32::WHITE,
                    );
                    if globe {
                        super::super::admin_globe::paint(
                            &painter,
                            &layout,
                            &model.globe_view,
                            &batch,
                            shade,
                            0.3,
                            model.admin_options.layer_opacity,
                        );
                    } else {
                        super::super::admin_render::paint(
                            &painter,
                            &data,
                            &model.admin_options,
                            model.map_theme.admin_palette(),
                            focus.lon,
                            local_project,
                        );
                    }
                },
            );
            for (id, delta) in &output.textures_delta.set {
                renderer.update_texture(&device, &queue, *id, delta);
            }
            let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
            let mut encoder = device.create_command_encoder(&Default::default());
            let commands = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.025,
                                g: 0.035,
                                b: 0.045,
                                a: 1.,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
            }
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 1024 * 768 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            if frame == 2 {
                encoder.copy_texture_to_buffer(
                    target.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &readback,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(4096),
                            rows_per_image: None,
                        },
                    },
                    target.size(),
                );
            }
            queue.submit(
                commands
                    .into_iter()
                    .chain(std::iter::once(encoder.finish())),
            );
            device.poll(wgpu::Maintain::Wait);
            if frame == 2 {
                let (tx, rx) = std::sync::mpsc::channel();
                readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                device.poll(wgpu::Maintain::Wait);
                rx.recv().unwrap().unwrap();
                pictures.push(readback.slice(..).get_mapped_range().to_vec());
            }
        }
    }
    let pixel =
        |i: usize, x: usize, y: usize| &pictures[i][(y * 1024 + x) * 4..(y * 1024 + x + 1) * 4];
    let (inside, outside) = if real {
        (
            GeoPoint {
                lat: 42.55,
                lon: 1.58,
            },
            GeoPoint {
                lat: 42.70,
                lon: 1.58,
            },
        )
    } else {
        (
            GeoPoint { lat: 1., lon: 1. },
            GeoPoint { lat: 2.5, lon: 2.5 },
        )
    };
    let (interior, hole) = if globe {
        let front = crate::model::GlobeViewState::from_focus(focus);
        let screen = |lat, lon| {
            super::super::globe_scene::project_geo(&layout, &front, GeoPoint { lat, lon }, 0.)
                .unwrap()
                .pos
        };
        (
            screen(inside.lat, inside.lon),
            screen(outside.lat, outside.lon),
        )
    } else {
        (local_project(inside), local_project(outside))
    };
    assert_ne!(
        pixel(0, interior.x as usize, interior.y as usize),
        pixel(1, interior.x as usize, interior.y as usize),
        "region interior shades"
    );
    assert_eq!(
        pixel(0, hole.x as usize, hole.y as usize),
        pixel(1, hole.x as usize, hole.y as usize),
        "hole remains unshaded"
    );
    if globe {
        for y in 0..768 {
            assert_eq!(
                &pictures[3][y * 4096..y * 4096 + 780 * 4],
                &pictures[4][y * 4096..y * 4096 + 780 * 4],
                "hidden hemisphere never shades"
            );
        }
    }
    for y in 0..768 {
        assert_eq!(
            &pictures[0][y * 4096..y * 4096 + 780 * 4],
            &pictures[2][y * 4096..y * 4096 + 780 * 4]
        );
        assert_eq!(
            &pictures[1][y * 4096..y * 4096 + 780 * 4],
            &pictures[9][y * 4096..y * 4096 + 780 * 4],
            "restoring opacity restores the exact shaded map"
        );
        assert_eq!(
            &pictures[6][y * 4096..y * 4096 + 780 * 4],
            &pictures[7][y * 4096..y * 4096 + 780 * 4],
            "zero opacity hides shading"
        );
    }
    let background = pixel(6, 10, 400);
    let difference = |p: &[u8]| {
        p.iter()
            .zip(background)
            .take(3)
            .map(|(&a, &b)| u64::from(a.abs_diff(b)))
            .sum::<u64>()
    };
    let outline_energy = |i| {
        (100..700)
            .flat_map(|y| (20..760).map(move |x| (x, y)))
            .map(|(x, y)| difference(pixel(i, x, y)))
            .sum::<u64>()
    };
    assert_eq!(outline_energy(6), 0, "zero opacity hides every boundary");
    assert!(
        outline_energy(8) > 0 && outline_energy(8) < outline_energy(0),
        "half opacity visibly fades outlines"
    );
    let interior_energy = |i| difference(pixel(i, interior.x as usize, interior.y as usize));
    assert!(
        interior_energy(5) > 0 && interior_energy(5) < interior_energy(1),
        "half opacity visibly fades shading"
    );
    if let Ok(path) = std::env::var(if globe {
        "ONEKEE_ADMIN_GLOBE_PREVIEW"
    } else {
        "ONEKEE_ADMIN_PREVIEW"
    }) {
        image::save_buffer(path, &pictures[1], 1024, 768, image::ColorType::Rgba8).unwrap();
    }
    if real {
        // Check borders independently of the fill comparison and UI text.
        let background = pixel(0, 10, 400);
        let outline_pixels = (100..700)
            .flat_map(|y| (20..760).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(0, x, y) != background)
            .count();
        assert!(outline_pixels > 100, "real boundary outline is visible");
    }
    pixel(1, interior.x as usize, interior.y as usize)
        .try_into()
        .unwrap()
}
