//! Paint a native egui brief once. Both eyes sample the same panel in room space.
use crate::scene::Marker;

pub fn texture(device: &wgpu::Device, queue: &wgpu::Queue, marker: &Marker) -> wgpu::TextureView {
    let size = [1000, 400];
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let panel = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("event brief"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            format,
            dimension: wgpu::TextureDimension::D2,
            mip_level_count: 1,
            sample_count: 1,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default());
    let ctx = egui::Context::default();
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(500., 200.),
        )),
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(2.);
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(14, 26, 35))
                    .inner_margin(16),
            )
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("{:.4}°   {:.4}°", marker.lat, marker.lon))
                        .size(16.)
                        .color(egui::Color32::LIGHT_BLUE),
                );
                ui.add(
                    egui::Label::new(egui::RichText::new(&marker.title).strong().size(25.))
                        .truncate(),
                );
                ui.add_space(4.);
                ui.label(egui::RichText::new(&marker.summary).size(17.));
            });
    });
    let jobs = ctx.tessellate(out.shapes, out.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: size,
        pixels_per_point: out.pixels_per_point,
    };
    let mut renderer = egui_wgpu::Renderer::new(device, format, None, 1, false);
    for (id, delta) in &out.textures_delta.set {
        renderer.update_texture(device, queue, *id, delta);
    }
    let mut enc = device.create_command_encoder(&Default::default());
    let mut commands = renderer.update_buffers(device, queue, &mut enc, &jobs, &screen);
    {
        let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui brief"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &panel,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
    }
    commands.push(enc.finish());
    queue.submit(commands);
    panel
}
