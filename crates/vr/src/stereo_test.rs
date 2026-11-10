//! Read back the actual stereo passes on a Vulkan GPU, independently of tracking.
use super::*;
use crate::scene::TileSource;
use glam::Vec3;

pub(crate) fn stereo(lean: f32) -> [xr::View; 2] {
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

pub(crate) fn target(device: &wgpu::Device, size: [u32; 2]) -> wgpu::Texture {
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
    let mut renderer = Renderer::new(
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

#[test]
#[ignore = "measures the native Local table and wrist on a Vulkan GPU"]
fn local_relief_and_wrist_remain_registered_while_native_map_uploads() {
    use crate::operator::{MAP_SIZE, MapImage, WRIST_SIZE};
    use glam::{Mat4, Quat};
    use one_thousand_electric_eye_desktop::vr_operator::{HEIGHT_EDGE, MapFrame};
    let (device, queue) = device();
    let marker = Marker::load(None).unwrap();
    let globe = Globe::place(&stereo(0.), &marker);
    let size = [2468, 2740];
    let textures = [target(&device, size), target(&device, size)];
    let views = textures
        .each_ref()
        .map(|t| t.create_view(&Default::default()));
    let mut renderer = Renderer::new(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &marker,
    )
    .unwrap();
    let hand = Mat4::from_scale_rotation_translation(
        Vec3::new(0.24, 0.2, 1.),
        Quat::IDENTITY,
        // Through the table from both eyes. Wrist controls must remain visible
        // when an operator lowers their hand into the geographic scene.
        Vec3::new(-0.15, -0.12, -1.22),
    );
    renderer.cartography.wrist_pose = Some(hand);
    let at = |u: f32| 0.08 + u * 0.14;
    let frame = MapFrame {
        local: true,
        heights: (0..HEIGHT_EDGE * HEIGHT_EDGE)
            .map(|i| at((i % HEIGHT_EDGE) as f32 / (HEIGHT_EDGE - 1) as f32))
            .collect(),
        ..Default::default()
    };
    let mut map = Vec::new();
    for y in 0..MAP_SIZE[1] {
        for x in 0..MAP_SIZE[0] {
            map.extend_from_slice(&[
                (x * 255 / (MAP_SIZE[0] - 1)) as u8,
                (y * 255 / (MAP_SIZE[1] - 1)) as u8,
                20,
                255,
            ]);
        }
    }
    let wrist = [19u8, 211, 140, 255].repeat((WRIST_SIZE[0] * WRIST_SIZE[1]) as usize);
    let mut timer = GpuTimer::new(&device).expect("GPU timestamps");
    let mut gpu = vec![];
    let mut cpu = vec![];
    for i in 0..140 {
        let start = std::time::Instant::now();
        renderer.cartography.advance(
            &queue,
            (i % 14 == 0).then(|| MapImage {
                pixels: map.clone(),
                frame: frame.clone(),
            }),
            (i == 0).then(|| wrist.clone()),
        );
        if i < 7 {
            assert!(renderer.cartography.frame.is_none());
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        for (eye, v) in stereo(0.06).into_iter().enumerate() {
            renderer.draw(
                &queue,
                &mut encoder,
                eye,
                &views[eye],
                size,
                v,
                &globe,
                &marker,
                false,
                None,
                timer.writes(eye),
            );
        }
        timer.resolve(&mut encoder);
        queue.submit([encoder.finish()]);
        timer.submitted();
        device.poll(wgpu::Maintain::Wait);
        let ms = timer.sample(queue.get_timestamp_period()).unwrap();
        if i >= 20 {
            gpu.push(ms);
            cpu.push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    for (eye, tex) in stereo(0.06).into_iter().zip(textures) {
        let vp = projection(eye.fov) * pose(eye.pose).inverse();
        for (p, expected) in [
            (
                globe
                    .table_transform()
                    .transform_point3(Vec3::new(0.2, 0.16, at(0.7))),
                [179, 87, 20],
            ),
            (hand.w_axis.truncate(), [19, 211, 140]),
        ] {
            let n = vp.project_point3(p);
            let origin = wgpu::Origin3d {
                x: ((1. + n.x) * 0.5 * size[0] as f32) as u32,
                y: ((1. - n.y) * 0.5 * size[1] as f32) as u32,
                z: 0,
            };
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut e = device.create_command_encoder(&Default::default());
            e.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    origin,
                    ..tex.as_image_copy()
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: Default::default(),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([e.finish()]);
            let (tx, rx) = mpsc::channel();
            buf.slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::Maintain::Wait);
            rx.recv().unwrap().unwrap();
            let mapped = buf.slice(..).get_mapped_range();
            assert!(
                mapped[..3]
                    .iter()
                    .zip(expected)
                    .all(|(&got, want)| got.abs_diff(want) < 5),
                "surface pixel {:?}, expected {expected:?}",
                &mapped[..3]
            );
        }
    }
    gpu.sort_by(f64::total_cmp);
    cpu.sort_by(f64::total_cmp);
    let n = (gpu.len() * 95).div_ceil(100) - 1;
    eprintln!(
        "Local relief with wrist and streamed map at 2468 × 2740 per eye: {:.2} ms stereo GPU p95, {:.2} ms CPU through GPU completion p95",
        gpu[n], cpu[n]
    );
}

/// Check pixels using geographic landmarks in each eye, independently of the capture shader.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_native_eyes(
    d: &wgpu::Device,
    q: &wgpu::Queue,
    renderer: &mut Renderer,
    globe: &Globe,
    marker: &Marker,
    head: Vec3,
    near: [f32; 2],
    side: usize,
) {
    use glam::{Mat4, Quat};
    let model = globe.transform();
    let center = model.w_axis.truncate();
    let camera = Mat4::look_at_rh(head, center, Vec3::Y).inverse();
    let rot = Quat::from_mat4(&camera);
    let local = surface(near[0], near[1]);
    let aim = model.transform_point3(local * 1.063);
    let start = camera.transform_point3(Vec3::new(0.22, -0.18, -0.18));
    let ray = Mat4::look_at_rh(start, aim, Vec3::Y).inverse();
    let pick = renderer.cartography.spires.pick(
        model.inverse() * ray,
        f32::INFINITY,
        renderer.cartography.frame.as_ref().unwrap(),
    );
    assert_eq!(pick.map(|p| p.0), Some(0));
    renderer.cartography.laser = Some((start, aim));
    renderer.cartography.aiming_at_target = true;
    const W: u32 = 1024;
    let mut preview = image::RgbaImage::new(2 * W, W);
    for (i, mut eye) in stereo(0.).into_iter().enumerate() {
        let p = camera.transform_point3(Vec3::new(eye.pose.position.x, 0., 0.));
        eye.pose.position = xr::Vector3f {
            x: p.x,
            y: p.y,
            z: p.z,
        };
        eye.pose.orientation = xr::Quaternionf {
            x: rot.x,
            y: rot.y,
            z: rot.z,
            w: rot.w,
        };
        let img = read_native_eye(d, q, renderer, globe, marker, eye, i);
        image::imageops::replace(&mut preview, &img, i as i64 * i64::from(W), 0);
        if let Some(dir) = std::env::var_os("ONEKEE_VR_CAPTURE_DIR") {
            preview
                .save(std::path::Path::new(&dir).join(format!("hemisphere-{side}.png")))
                .unwrap();
        }
        let vp = projection(eye.fov) * pose(eye.pose).inverse();
        let pixel = |p| {
            let v = vp.project_point3(p);
            (
                ((v.x + 1.) * 0.5 * W as f32) as u32,
                ((1. - v.y) * 0.5 * W as f32) as u32,
            )
        };
        let around = |point, pred: fn(&[u8; 4]) -> bool| {
            let (x, y) = pixel(point);
            (x - 2..=x + 2).any(|x| (y - 2..=y + 2).any(|y| pred(&img.get_pixel(x, y).0)))
        };
        let main_lon = near[1] - 32.;
        for lat in [-15., 8., 25., 42.] {
            for offset in [-34., -12., 17., 38.] {
                let (x, y) = pixel(model.transform_point3(surface(lat, main_lon + offset)));
                let p = img.get_pixel(x, y).0;
                let warm = (main_lon + offset < 0.) == (lat > 0.);
                assert!(
                    if warm {
                        p[0] > p[2].saturating_add(25)
                    } else {
                        p[2] > p[0].saturating_add(25)
                    },
                    "eye {i} lat {lat} lon {} expected warm={warm} got {p:?}",
                    main_lon + offset
                );
            }
        }
        assert!(
            around(model.transform_point3(local * 1.025), |p| p[0] > 190
                && p[1] < 165
                && p[2] > 190),
            "radial spire missing in eye {i}"
        );
        assert!(
            around(start.lerp(aim, 0.43), |p| p[1] > 230
                && p[0] < 180
                && p[2] > 175),
            "laser missing in eye {i}"
        );
        assert!(
            !img.pixels().any(|p| p[1] > 240 && p[0] < 40 && p[2] < 40),
            "back-side spire visible through globe"
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn read_native_eye(
    d: &wgpu::Device,
    q: &wgpu::Queue,
    renderer: &mut Renderer,
    globe: &Globe,
    marker: &Marker,
    eye: xr::View,
    i: usize,
) -> image::RgbaImage {
    const W: u32 = 1024;
    let tex = target(d, [W, W]);
    let mut e = d.create_command_encoder(&Default::default());
    renderer.draw(
        q,
        &mut e,
        i,
        &tex.create_view(&Default::default()),
        [W, W],
        eye,
        globe,
        marker,
        false,
        None,
        None,
    );
    let buf = d.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (W * W * 4).into(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    e.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                bytes_per_row: Some(W * 4),
                ..Default::default()
            },
        },
        tex.size(),
    );
    q.submit([e.finish()]);
    let (tx, rx) = mpsc::channel();
    buf.slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    d.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().unwrap();
    let img = image::RgbaImage::from_raw(W, W, buf.slice(..).get_mapped_range().to_vec()).unwrap();
    buf.unmap();
    img
}
