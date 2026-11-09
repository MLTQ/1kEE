use crate::scene::{Globe, Marker, TileSource};
use crate::vulkan::{Eye, Graphics};
use crate::{input, render};
use anyhow::{Context, Result, bail, ensure};
use ash::vk;
use openxr as xr;
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
const STEREO: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

#[derive(Default)]
struct Options {
    frames: Option<u64>,
    reference: bool,
    root: Option<PathBuf>,
    import: Option<PathBuf>,
    tile: Option<TileSource>,
    stream_after: u64,
    display_test: bool,
    events: Option<PathBuf>,
}
fn options() -> Result<Option<Options>> {
    let mut o = Options {
        tile: Some(TileSource::Bundled),
        stream_after: 120,
        ..Default::default()
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "1kEE native OpenXR globe\n\nStart SteamVR with the Index tracking, then run this binary.\n\n  --import FILE       Open a georeferenced GeoTIFF, GeoJSON, KML or KMZ layer\n  --asset-root PATH   Override the desktop asset root for native map tiles\n  --demo              Run the original single-tile stereo diagnostic\n  --frames N          Exit after N rendered stereo frames\n  --stream-after N    Start reading the contour tile after N rendered frames (default 120)\n  --display-test      Use head-relative stereo for display diagnostics only\n  --no-terrain        Baseline without contour work\n  --tile DB Z Y X     Use a tile from a native 1kEE contour SQLite cache\n  --events DB         Focus the latest geolocated event in a desktop event database\n\nLeft A hides/shows wrist controls. Aim and trigger with the right hand. Right stick scrolls over menus and zooms over the map."
                );
                return Ok(None);
            }
            "--demo" => o.reference = true,
            "--import" => o.import = Some(value()?.into()),
            "--asset-root" => o.root = Some(value()?.into()),
            "--display-test" => o.display_test = true,
            "--frames" => {
                let n = value()?.parse()?;
                ensure!(n > 0, "--frames must be positive");
                o.frames = Some(n);
            }
            "--stream-after" => {
                o.reference = true;
                o.stream_after = value()?.parse()?;
            }
            "--events" => o.events = Some(value()?.into()),
            "--no-terrain" => {
                o.reference = true;
                o.tile = None;
            }
            "--tile" => {
                o.reference = true;
                o.tile = Some(TileSource::Native {
                    path: value()?.into(),
                    key: [value()?.parse()?, value()?.parse()?, value()?.parse()?],
                })
            }
            _ => bail!("unknown option {arg}. Use --help."),
        }
    }
    Ok(Some(o))
}

struct Terrain {
    source: Option<TileSource>,
    receiver: Option<mpsc::Receiver<Result<Vec<[f32; 3]>>>>,
    upload: Option<Vec<[f32; 3]>>,
    start: Option<Instant>,
}
impl Terrain {
    fn advance(&mut self, renderer: &mut render::Renderer, queue: &wgpu::Queue) -> Result<()> {
        if let Some(source) = self.source.take() {
            let (tx, rx) = mpsc::sync_channel(1);
            self.start = Some(Instant::now());
            std::thread::Builder::new()
                .name("vr-contour-reader".into())
                .spawn(move || {
                    let _ = tx.send(source.read());
                })?;
            self.receiver = Some(rx);
        }
        if let Some(rx) = &self.receiver {
            match rx.try_recv() {
                Ok(v) => {
                    self.upload = Some(v?);
                    self.receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => bail!("contour worker stopped"),
            }
        }
        if let Some(data) = &self.upload
            && renderer.upload_chunk(queue, data)
        {
            eprintln!(
                "Contour tile resident: {} segments, {:.1} ms from request to submission",
                data.len() / 2,
                self.start.unwrap().elapsed().as_secs_f64() * 1e3
            );
            self.upload = None;
        }
        Ok(())
    }
}

#[derive(Default)]
struct Timing {
    cpu: Vec<f64>,
    gpu: Vec<f64>,
    gaps: u64,
    previous: Option<xr::Time>,
}
impl Timing {
    fn record(&mut self, time: xr::Time, period: xr::Duration, cpu: Duration, gpu: Option<f64>) {
        if let Some(last) = self.previous
            && (time - last).as_nanos() > period.as_nanos() * 3 / 2
        {
            self.gaps += 1;
        }
        self.previous = Some(time);
        self.cpu.push(cpu.as_secs_f64() * 1000.);
        if let Some(g) = gpu {
            self.gpu.push(g);
        }
    }
    fn report(&mut self, frame: u64, period: xr::Duration) {
        fn p95(v: &mut [f64]) -> Option<f64> {
            v.sort_by(f64::total_cmp);
            v.get((v.len() * 95).div_ceil(100).saturating_sub(1))
                .copied()
        }
        let gpu = p95(&mut self.gpu).map_or_else(|| "unavailable".into(), |v| format!("{v:.2} ms"));
        eprintln!(
            "frame {frame}: {} frames, period {:.2} ms, app CPU p95 {:.2} ms, stereo GPU p95 {gpu}, predicted display gaps {}",
            self.cpu.len(),
            period.as_nanos() as f64 / 1e6,
            p95(&mut self.cpu).unwrap_or(0.),
            self.gaps
        );
        self.cpu.clear();
        self.gpu.clear();
        self.gaps = 0;
    }
}

fn run(o: Options) -> Result<()> {
    let marker = Marker::load(o.events.as_deref())?;
    if o.reference {
        eprintln!(
            "Marker: {} ({:.4}, {:.4})",
            marker.title, marker.lat, marker.lon
        );
    } else {
        eprintln!(
            "Native operator starting at {:.4}°, {:.4}°",
            marker.lat, marker.lon
        );
    }
    // The loader, Vulkan device and session all outlive their dependent objects.
    let entry = unsafe { xr::Entry::load(&()) }
        .context("load libopenxr_loader; install the OpenXR loader")?;
    ensure!(
        entry.enumerate_extensions()?.khr_vulkan_enable,
        "runtime lacks XR_KHR_vulkan_enable"
    );
    let mut extensions = xr::ExtensionSet::default();
    extensions.khr_vulkan_enable = true;
    let instance = entry
        .create_instance(
            &xr::ApplicationInfo {
                application_name: "1kEE VR",
                application_version: 1,
                engine_name: "1kEE wgpu",
                engine_version: 1,
                api_version: xr::Version::new(1, 0, 0),
            },
            &extensions,
            &[],
            &(),
        )
        .context("open the OpenXR runtime; start SteamVR first")?;
    eprintln!("OpenXR runtime: {}", instance.properties()?.runtime_name);
    let system = instance
        .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
        .context("SteamVR must see a connected, ready headset")?;
    eprintln!(
        "Headset: {}",
        instance.system_properties(system)?.system_name
    );
    let gpu = Graphics::new(&instance, system)?;
    let (session, mut waiter, mut stream) =
        unsafe { instance.create_session::<xr::Vulkan>(system, &gpu.binding) }?;
    let mut controls = input::Controls::new(&instance, &session)?;
    let reference = if o.display_test {
        eprintln!("Display test: head-relative projection, independent of room tracking");
        xr::ReferenceSpaceType::VIEW
    } else {
        xr::ReferenceSpaceType::LOCAL
    };
    let space = session.create_reference_space(reference, xr::Posef::IDENTITY)?;
    let formats = session.enumerate_swapchain_formats()?;
    let (raw, format) = [
        (
            vk::Format::R8G8B8A8_SRGB,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ),
        (
            vk::Format::B8G8R8A8_SRGB,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ),
    ]
    .into_iter()
    .find(|(f, _)| formats.contains(&(f.as_raw() as _)))
    .context("no sRGB eye swapchain format")?;
    let config = instance.enumerate_view_configuration_views(system, STEREO)?;
    ensure!(config.len() == 2, "expected two primary stereo views");
    let mut eyes = Vec::new();
    for v in config {
        let size = [
            v.recommended_image_rect_width,
            v.recommended_image_rect_height,
        ];
        eprintln!("Eye target: {} × {}", size[0], size[1]);
        eyes.push(Eye::new(&gpu.device, &session, size, raw, format)?);
    }
    let mut renderer = render::Renderer::new(&gpu.device, &gpu.queue, format, &marker)?;
    let mut cockpit = if o.reference {
        None
    } else {
        let link = crate::operator::Link::start(
            gpu.adapter_name.clone(),
            marker.lat,
            marker.lon,
            o.root,
            o.events,
            o.import,
        )?;
        Some(crate::cockpit::Cockpit::new(
            link,
            one_thousand_electric_eye_desktop::vr_operator::View {
                lat: marker.lat,
                lon: marker.lon,
                scale: 1.,
                local: false,
                local_zoom: 25.,
            },
        ))
    };
    let mut timer = render::GpuTimer::new(&gpu.device);
    let mut terrain = Terrain {
        source: o.tile,
        receiver: None,
        upload: None,
        start: None,
    };
    let mut timing = Timing::default();
    let mut running = false;
    let mut focused = false;
    let mut event = xr::EventDataBuffer::new();
    let mut globe = None;
    let mut recenter_at = None;
    let mut count = 0;
    let mut waiting = None;
    let mut last_render = Instant::now();
    loop {
        while let Some(e) = instance.poll_event(&mut event)? {
            match e {
                xr::Event::SessionStateChanged(e) => {
                    eprintln!("Session: {:?}", e.state());
                    focused = e.state() == xr::SessionState::FOCUSED;
                    match e.state() {
                        xr::SessionState::READY => {
                            session.begin(STEREO)?;
                            running = true;
                        }
                        xr::SessionState::STOPPING => {
                            session.end()?;
                            running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                            ensure!(
                                o.frames.is_none_or(|n| count >= n),
                                "session ended after {count} rendered frames"
                            );
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => {
                    bail!("OpenXR runtime lost after {count} rendered frames")
                }
                xr::Event::ReferenceSpaceChangePending(change)
                    if change.reference_space_type() == reference =>
                {
                    recenter_at = Some(change.change_time());
                }
                _ => {}
            }
        }
        if o.frames.is_some() && last_render.elapsed() > Duration::from_secs(30) {
            bail!(
                "No stereo frames for 30 seconds (rendered {count}). Check SteamVR headset visibility, standby and base station tracking."
            );
        }
        if !running {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let frame = waiter.wait()?;
        let started = Instant::now();
        stream.begin()?;
        let time = frame.predicted_display_time;
        let blend = xr::EnvironmentBlendMode::OPAQUE;
        if !frame.should_render {
            if waiting != Some("visibility") {
                eprintln!("Waiting for SteamVR to request visible frames");
                waiting = Some("visibility");
            }
            stream.end(time, blend, &[])?;
            timing.previous = None;
            continue;
        }
        let (flags, views) = session.locate_views(STEREO, time, &space)?;
        if views.len() != 2
            || !flags.contains(
                xr::ViewStateFlags::POSITION_VALID | xr::ViewStateFlags::ORIENTATION_VALID,
            )
        {
            if waiting != Some("tracking") {
                eprintln!(
                    "Waiting for valid eye poses ({flags:?}); wake the Index and check base station visibility"
                );
                waiting = Some("tracking");
            }
            stream.end(time, blend, &[])?;
            timing.previous = None;
            continue;
        }
        waiting = None;
        if recenter_at.is_some_and(|change| time.as_nanos() >= change.as_nanos()) {
            globe = None;
            recenter_at = None;
        }
        let globe = globe.get_or_insert_with(|| Globe::place(&views, &marker));
        let hands = if focused {
            controls.sample(&session, &space, time)?
        } else {
            Default::default()
        };
        let dt = (frame.predicted_display_period.as_nanos() as f32 / 1e9).clamp(0., 0.1);
        if let Some(operator) = &mut cockpit {
            let head = (crate::scene::pose(views[0].pose).w_axis
                + crate::scene::pose(views[1].pose).w_axis)
                .truncate()
                * 0.5;
            let (reset, exit) =
                operator.tick(&mut renderer, &gpu.queue, globe, &hands, head, dt)?;
            controls.cursor = operator.cursor;
            if reset {
                globe.recenter(&views, &marker);
            }
            if exit {
                session.request_exit()?;
            }
        } else {
            globe.rotate(hands.rotate.x, hands.rotate.y, dt);
            globe.zoom(hands.zoom, dt);
            controls.cursor = hands.right.and_then(|a| globe.aim_point(a));
            if hands.click {
                controls.selected = controls.cursor.is_some_and(|p| {
                    p.distance(crate::scene::surface(marker.lat, marker.lon)) * globe.scale < 0.045
                });
            }
            if count >= o.stream_after
                && let Err(e) = terrain.advance(&mut renderer, &gpu.queue)
            {
                eprintln!("Contour tile unavailable: {e:#}");
                terrain.receiver = None;
                terrain.upload = None;
            }
        }
        gpu.device.poll(wgpu::Maintain::Poll);
        let measured = timer
            .as_mut()
            .and_then(|t| t.sample(gpu.queue.get_timestamp_period()));
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        for (index, eye) in eyes.iter_mut().enumerate() {
            let size = eye.size;
            let target = eye.acquire()?;
            renderer.draw(
                &gpu.queue,
                &mut encoder,
                index,
                target,
                size,
                views[index],
                globe,
                &marker,
                controls.selected,
                if focused { controls.cursor } else { None },
                timer.as_ref().and_then(|t| t.writes(index)),
            );
        }
        if let Some(t) = &timer {
            t.resolve(&mut encoder);
        }
        gpu.queue.submit([encoder.finish()]);
        if let Some(t) = &mut timer {
            t.submitted();
        }
        for eye in &mut eyes {
            eye.chain.release_image()?;
        }
        let projections: Vec<_> = eyes
            .iter()
            .zip(&views)
            .map(|(eye, view)| {
                xr::CompositionLayerProjectionView::new()
                    .pose(view.pose)
                    .fov(view.fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&eye.chain)
                            .image_rect(xr::Rect2Di {
                                offset: xr::Offset2Di { x: 0, y: 0 },
                                extent: xr::Extent2Di {
                                    width: eye.size[0] as _,
                                    height: eye.size[1] as _,
                                },
                            }),
                    )
            })
            .collect();
        stream.end(
            time,
            blend,
            &[&xr::CompositionLayerProjection::new()
                .space(&space)
                .views(&projections)],
        )?;
        count += 1;
        last_render = Instant::now();
        timing.record(
            time,
            frame.predicted_display_period,
            started.elapsed(),
            measured,
        );
        if count % 120 == 0 || o.frames == Some(count) {
            timing.report(count, frame.predicted_display_period);
        }
        if o.frames == Some(count) {
            session.request_exit()?;
        }
    }
}

pub fn main() -> Result<()> {
    if let Some(options) = options()? {
        run(options)?;
    }
    Ok(())
}
