//! Read back the actual stereo passes on a Vulkan GPU, independently of tracking.
use super::*;
use crate::scene::TileSource;
use glam::Vec3;

fn stereo(lean: f32) -> [xr::View; 2] {
    [-1., 1.].map(|side| {
        let mut pose = xr::Posef::IDENTITY;
        pose.position.x = lean + side * 0.032;
        xr::View {
            pose,
            fov: xr::Fovf {
                angle_left: -0.82 + side * 0.07,
                angle_right: 0.82 + side * 0.07,
                angle_up: 0.82,
                angle_down: -0.82,
            },
        }
    })
}

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    let name = std::env::var("ONEKEE_GPU").unwrap_or_default();
    let adapter = instance
        .enumerate_adapters(wgpu::Backends::VULKAN)
        .into_iter()
        .find(|a| a.get_info().name.contains(&name))
        .expect("requested Vulkan GPU");
    eprintln!("offscreen GPU: {}", adapter.get_info().name);
    pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
            ..Default::default()
        },
        None,
    ))
    .unwrap()
}

fn target(device: &wgpu::Device, size: [u32; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test eye"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn real_stereo_pixels_match_geographic_marker_and_hide_it_beyond_limb() {
    let (device, queue) = device();
    let marker = Marker::load(None).unwrap();
    let renderer = Renderer::new(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &marker,
    )
    .unwrap();
    let mut globe = Globe::place(&stereo(0.), &marker);
    const W: u32 = 512;
    let image = target(&device, [W, W]);
    let view = image.create_view(&Default::default());
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (W * W * 4) as _,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut preview = image::RgbaImage::new(W * 2, W);
    for (hidden, selected) in [(false, false), (false, true), (true, false)] {
        if hidden {
            globe.lon += 180.;
        }
        for (index, eye) in stereo(0.12).into_iter().enumerate() {
            let mut enc = device.create_command_encoder(&Default::default());
            renderer.draw(
                &queue,
                &mut enc,
                index,
                &view,
                [W, W],
                eye,
                &globe,
                &marker,
                selected,
                None,
                None,
            );
            enc.copy_texture_to_buffer(
                image.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(W * 4),
                        rows_per_image: None,
                    },
                },
                image.size(),
            );
            queue.submit([enc.finish()]);
            let (tx, rx) = mpsc::channel();
            staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::Maintain::Wait);
            rx.recv().unwrap().unwrap();
            let buf = staging.slice(..).get_mapped_range();
            if selected {
                let eye_image = image::RgbaImage::from_raw(W, W, buf.to_vec()).unwrap();
                let bright_letters = eye_image
                    .rows()
                    .take((W / 3) as usize)
                    .flatten()
                    .filter(|p| p[0] > 160 && p[1] > 160 && p[2] > 160)
                    .count();
                assert!(
                    bright_letters > 15,
                    "selected egui brief has no readable text"
                );
                image::imageops::replace(&mut preview, &eye_image, (index as u32 * W) as _, 0);
                drop(buf);
                staging.unmap();
                continue;
            }
            let mut sum = Vec3::ZERO;
            for (i, p) in buf.as_chunks::<4>().0.iter().enumerate() {
                if p[0] > 200 && p[1] > 100 && p[2] < 135 {
                    sum += Vec3::new((i as u32 % W) as f32 + 0.5, (i as u32 / W) as f32 + 0.5, 1.);
                }
            }
            drop(buf);
            staging.unmap();
            if hidden {
                assert_eq!(sum.z, 0.);
                continue;
            }
            assert!(sum.z > 3.);
            let m = projection(eye.fov) * pose(eye.pose).inverse() * globe.transform();
            let ndc = m.project_point3(surface(marker.lat, marker.lon));
            assert!((sum.x / sum.z - (ndc.x + 1.) * W as f32 * 0.5).abs() < 2.);
            assert!((sum.y / sum.z - (1. - ndc.y) * W as f32 * 0.5).abs() < 2.);
        }
    }
    if let Some(path) = std::env::var_os("ONEKEE_VR_STEREO_PNG") {
        preview.save(path).unwrap();
    }
}

#[test]
#[ignore = "measures two full-resolution eye draws on a Vulkan GPU"]
fn measure_resident_and_streaming_stereo_gpu_time() {
    let (device, queue) = device();
    let marker = Marker::load(None).unwrap();
    let globe = Globe::place(&stereo(0.), &marker);
    let size = [2468, 2740];
    let textures = [target(&device, size), target(&device, size)];
    let images = textures
        .each_ref()
        .map(|t| t.create_view(&Default::default()));
    let mut renderer = Renderer::new(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &marker,
    )
    .unwrap();
    let mut timer = GpuTimer::new(&device).expect("GPU timestamps");
    let tile = TileSource::Bundled.read().unwrap();
    let mut complete = false;
    let mut samples = [Vec::new(), Vec::new()];
    let mut wall = [Vec::new(), Vec::new()];
    for frame in 0..240 {
        let start = std::time::Instant::now();
        let mut enc = device.create_command_encoder(&Default::default());
        if frame >= 120 && !complete {
            complete = renderer.upload_chunk(&queue, &tile);
        }
        for (i, eye) in stereo((frame as f32 * 0.04).sin() * 0.08)
            .into_iter()
            .enumerate()
        {
            renderer.draw(
                &queue,
                &mut enc,
                i,
                &images[i],
                size,
                eye,
                &globe,
                &marker,
                true,
                None,
                timer.writes(i),
            );
        }
        timer.resolve(&mut enc);
        queue.submit([enc.finish()]);
        timer.submitted();
        device.poll(wgpu::Maintain::Wait);
        let ms = timer.sample(queue.get_timestamp_period()).unwrap();
        if frame >= 10 {
            samples[usize::from(frame >= 120)].push(ms);
            wall[usize::from(frame >= 120)].push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    // Check the final full-size targets independently of the timestamp queries.
    // A small rectangle on the selected marker should contain green in both eyes.
    for (eye, texture) in stereo((239.0_f32 * 0.04).sin() * 0.08)
        .into_iter()
        .zip(textures)
    {
        let m = projection(eye.fov) * pose(eye.pose).inverse() * globe.transform();
        let ndc = m.project_point3(surface(marker.lat, marker.lon));
        let x = ((1. + ndc.x) * 0.5 * size[0] as f32) as u32;
        let y = ((1. - ndc.y) * 0.5 * size[1] as f32) as u32;
        let stage = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d { x: x - 4, y, z: 0 },
                ..texture.as_image_copy()
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &stage,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: None,
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: 8,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        let (tx, rx) = mpsc::channel();
        stage
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let range = stage.slice(..).get_mapped_range();
        assert!(
            range[..32]
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[1] > 200 && p[0] < 180),
            "full-size eye did not draw its marker: {:?}",
            &range[..32]
        );
    }
    assert!(complete);
    for ((phase, s), elapsed) in ["globe", "streaming/resident contours"]
        .into_iter()
        .zip(&mut samples)
        .zip(&mut wall)
    {
        s.sort_by(f64::total_cmp);
        elapsed.sort_by(f64::total_cmp);
        let n = s.len();
        eprintln!(
            "{phase} stereo GPU ({} pairs): median {:.2} ms, p95 {:.2} ms, max {:.2} ms; CPU through GPU completion p95 {:.2} ms",
            n,
            s[n / 2],
            s[(n * 95).div_ceil(100) - 1],
            s[n - 1],
            elapsed[(n * 95).div_ceil(100) - 1]
        );
    }
}
