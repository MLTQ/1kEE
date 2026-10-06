use super::super::super::super::globe_scene::GlobeLayout;
use super::super::gpu_tests::{draw, gpu, pixels, set_uniforms, target};
use super::*;

#[test]
#[ignore = "requires GPU; verifies progressive all-plane restoration and stable image reuse"]
fn stationary_refinement_restores_omitted_planes_then_reuses_the_image() {
    let (device, queue) = gpu();
    let mut tile = super::super::tests::tile(0., 2_200_001);
    for (i, p) in Arc::make_mut(&mut tile.contours)[0]
        .points
        .iter_mut()
        .enumerate()
    {
        p.lon = i as f32 * 0.0000001;
        p.lat *= 10.;
    }
    let frame = Arc::new(prepare(
        super::super::tests::frame(vec![tile]),
        None,
        egui::Color32::WHITE,
        egui::Color32::BLACK,
    ));
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(256., 128.));
    let view = GlobeViewState::from_focus(GeoPoint { lat: 0., lon: 0. });
    let layout = GlobeLayout {
        center: rect.center(),
        radius: 1000.,
        focal_length: 2.,
        camera_distance: 3.,
    };
    let viewport = Viewport::new(&layout, &view, rect);
    let mut gpu = Gpu::default();
    while !gpu.stage(&device, &frame, viewport) {}
    let res = ContourPassResources::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    set_uniforms(&queue, &res, &layout, &view, rect);
    let mut uniforms = ContourCallback::new(
        ContourLayer::SrtmGlobe,
        0,
        Arc::new(vec![]),
        &layout,
        &view,
        0.02,
        0.92,
        1.,
        1.,
    )
    .uniforms;
    uniforms.viewport_size = [256., 128.];
    let reference = target(&device, [256, 128]);
    let chunks: Vec<_> = frame
        .tiles
        .iter()
        .flat_map(|t| t.chunks.iter().zip(&gpu.tiles[&t.version]))
        .collect();
    draw(
        &device,
        &queue,
        &res,
        &reference,
        &chunks,
        4,
        Some(viewport),
        1,
    );
    let expected = pixels(&device, &queue, &reference);
    assert!(
        expected.chunks(4).any(|p| p[0] > 0),
        "reference fixture draws visible pixels"
    );
    let mut image = Cache::new(&device, res.format);
    let step = |image: &mut Cache, uniforms: &ContourUniforms| {
        let mut encoder = device.create_command_encoder(&Default::default());
        let pending = image.prepare(
            &device,
            &mut encoder,
            res.format,
            &res.pipeline,
            &res.bind_group,
            &frame,
            &gpu.tiles,
            uniforms,
            viewport,
            400,
        );
        queue.submit([encoder.finish()]);
        device.poll(wgpu::Maintain::Wait);
        pending
    };
    assert!(step(&mut image, &uniforms)); // preview excludes the 200m plane
    assert!(
        pixels(&device, &queue, &image.preview.as_ref().unwrap().texture)
            .chunks(4)
            .all(|p| p[0] == 0)
    );
    assert!(step(&mut image, &uniforms)); // first bounded refinement is incomplete
    assert!(image.next_chunk > 0 && !image.complete);
    assert!(!step(&mut image, &uniforms));
    assert!(image.complete);
    let full = pixels(&device, &queue, &image.full.as_ref().unwrap().texture);
    assert!(
        full.chunks(4).any(|p| p[0] > 0),
        "omitted preview plane restored"
    );
    assert!(
        full.chunks(4)
            .zip(expected.chunks(4))
            .all(|(a, b)| a[..3] == b[..3]),
        "refinement preserves every full-detail pixel"
    );

    let cursor = image.next_chunk;
    assert!(!step(&mut image, &uniforms));
    assert_eq!(image.next_chunk, cursor);
    assert_eq!(
        pixels(&device, &queue, &image.full.as_ref().unwrap().texture),
        full
    );
    uniforms.radius_focal *= 1.1;
    assert!(step(&mut image, &uniforms));
    assert!(!image.complete);
    assert_eq!(image.next_chunk, 0);
}
