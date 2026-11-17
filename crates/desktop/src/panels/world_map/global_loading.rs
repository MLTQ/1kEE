//! Frame-scoped, in-memory Globe loading indicators. Time animates activity,
//! never completion; source readiness drives the bar and pending-tile outlines.
use super::globe_scene::{GlobeLayout, project_geo};
use crate::{
    model::{GeoPoint, GlobeViewState},
    theme,
};
use std::time::Duration;

#[derive(Clone, Default, Debug)]
pub(crate) struct Snapshot {
    pub total: usize,
    pub ready: usize,
    pub fraction_sum: f32,
    pub building: usize,
    pub reading: usize,
    pub waiting: usize,
    pub preparing: usize,
    pub uploading: usize,
    pub failed: usize,
    pub selecting: usize,
    pub bounds: Vec<[f32; 4]>,
}
impl Snapshot {
    pub fn add(&mut self, other: Self) {
        self.total += other.total;
        self.ready += other.ready;
        self.fraction_sum += other.fraction_sum;
        self.building += other.building;
        self.reading += other.reading;
        self.waiting += other.waiting;
        self.preparing += other.preparing;
        self.uploading += other.uploading;
        self.failed += other.failed;
        self.selecting += other.selecting;
        self.bounds.extend(
            other
                .bounds
                .into_iter()
                .take(32usize.saturating_sub(self.bounds.len())),
        );
    }
    pub fn active(&self) -> bool {
        self.ready < self.total || self.preparing + self.uploading + self.selecting > 0
    }
    pub fn running(&self) -> bool {
        self.selecting + self.preparing + self.uploading > 0
            || self.total.saturating_sub(self.ready) > self.failed
    }
    pub fn fraction(&self) -> f32 {
        let f = if self.total > 0 {
            self.fraction_sum / self.total as f32
        } else {
            0.
        };
        f.clamp(0., if self.active() { 0.99 } else { 1. })
    }
}
fn id() -> egui::Id {
    egui::Id::new("globe-loading-frame")
}
pub(super) fn begin(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(id(), Snapshot::default()));
}
pub(crate) fn report(ctx: &egui::Context, snapshot: Snapshot) {
    ctx.data_mut(|d| {
        let current = d.get_temp_mut_or_default::<Snapshot>(id());
        current.add(snapshot);
    });
}
pub(super) fn watch_gpu(ctx: &egui::Context, slot: usize, version: u64) {
    let ready = gpu_ready(ctx, slot, version);
    if !ready {
        report(
            ctx,
            Snapshot {
                uploading: 1,
                ..Default::default()
            },
        );
    }
}
pub(super) fn gpu_ready(ctx: &egui::Context, slot: usize, version: u64) -> bool {
    ctx.data(|d| d.get_temp::<u64>(egui::Id::new(("globe-uploaded-layer", slot))) == Some(version))
}
pub(super) fn uploaded(ctx: &egui::Context, slot: usize, version: u64) {
    let changed = ctx.data_mut(|d| {
        let key = egui::Id::new(("globe-uploaded-layer", slot));
        let changed = d.get_temp::<u64>(key) != Some(version);
        d.insert_temp(key, version);
        changed
    });
    if changed {
        ctx.request_repaint();
    }
}
pub(super) fn pending_gpu(ctx: &egui::Context, slot: usize) {
    let changed = ctx.data_mut(|d| {
        let key = egui::Id::new(("globe-uploaded-layer", slot));
        let changed = d.get_temp::<u64>(key).is_some();
        d.remove::<u64>(key);
        changed
    });
    if changed {
        ctx.request_repaint();
    }
}
pub(super) fn draw(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    time: f64,
    animate: bool,
) {
    let snapshot = painter
        .ctx()
        .data(|d| d.get_temp::<Snapshot>(id()))
        .unwrap_or_default();
    paint(painter, rect, layout, view, &snapshot, time, animate);
}
fn paint(
    painter: &egui::Painter,
    rect: egui::Rect,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    s: &Snapshot,
    time: f64,
    animate: bool,
) {
    if !s.active() {
        return;
    }
    let running = s.running();
    painter
        .ctx()
        .request_repaint_after(Duration::from_millis(if animate && running {
            33
        } else {
            180
        }));
    let accent = if s.failed > 0 && !running {
        egui::Color32::from_rgb(219, 167, 91)
    } else {
        theme::topo_color()
    };
    if animate && running {
        let alpha = 0.20 + 0.15 * (time as f32 * 2.).sin();
        for bounds in s.bounds.iter().take(32) {
            outline(painter, layout, view, *bounds, accent.gamma_multiply(alpha));
        }
    }
    let width = (rect.width() - 24.).clamp(100., 350.);
    let frame = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 12. - width, rect.bottom() - 82.),
        egui::vec2(width, 70.),
    );
    painter.rect_filled(frame, 6., theme::panel_fill(230));
    painter.rect_stroke(
        frame,
        6.,
        egui::Stroke::new(1_f32, theme::panel_stroke()),
        egui::StrokeKind::Outside,
    );
    let title = if s.total == 0 {
        "GLOBE · FINDING TILES".to_string()
    } else {
        format!("GLOBE · DATA {} / {}", s.ready, s.total)
    };
    let label = format!(
        "{title}\nBUILD {} · LOAD {} · WAIT {}\nPREPARE {} · GPU {} · UNAVAILABLE {}",
        s.building, s.reading, s.waiting, s.preparing, s.uploading, s.failed
    );
    painter.text(
        frame.left_top() + egui::vec2(9., 7.),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::monospace(10.5),
        theme::text_muted(),
    );
    let bar = egui::Rect::from_min_size(
        frame.left_bottom() + egui::vec2(8., -11.),
        egui::vec2(width - 16., 5.),
    );
    painter.rect_filled(bar, 3., theme::panel_fill(230).gamma_multiply(2.5));
    if s.fraction() > 0. {
        painter.rect_filled(
            egui::Rect::from_min_size(
                bar.min,
                egui::vec2(bar.width() * s.fraction(), bar.height()),
            ),
            3.,
            accent,
        );
    }
    if animate && running {
        let center = frame.right_top() + egui::vec2(-18., 17.);
        let points = (0..=20)
            .map(|i| {
                let a = time as f32 * 3. + i as f32 / 20. * 4.5;
                center + egui::vec2(a.cos(), a.sin()) * 7.
            })
            .collect();
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.5_f32, accent),
        ));
    }
}
fn outline(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &GlobeViewState,
    [w, s, e, n]: [f32; 4],
    color: egui::Color32,
) {
    for (a, b) in [
        ([w, s], [e, s]),
        ([e, s], [e, n]),
        ([e, n], [w, n]),
        ([w, n], [w, s]),
    ] {
        let mut prior = None;
        for i in 0..=8 {
            let t = i as f32 / 8.;
            let point = project_geo(
                layout,
                view,
                GeoPoint {
                    lon: a[0] + (b[0] - a[0]) * t,
                    lat: a[1] + (b[1] - a[1]) * t,
                },
                0.,
            )
            .filter(|p| p.front_facing && p.pos.is_finite());
            if let (Some(a), Some(b)) = (prior, point) {
                painter.line_segment([a, b.pos], egui::Stroke::new(1_f32, color));
            }
            prior = point.map(|p| p.pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shapes(s: &Snapshot, time: f64, animate: bool) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 700.));
        let view = GlobeViewState::from_focus(GeoPoint { lon: 0., lat: 0. });
        let layout = super::super::globe_scene::globe_layout(rect, &view);
        ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                paint(
                    &ctx.layer_painter(egui::LayerId::background()),
                    rect,
                    &layout,
                    &view,
                    s,
                    time,
                    animate,
                );
            },
        )
        .shapes
    }
    #[test]
    fn globe_loading_paints_animation_only_while_allowed_and_keeps_static_progress() {
        let s = Snapshot {
            total: 4,
            ready: 1,
            fraction_sum: 1.,
            reading: 3,
            bounds: vec![[-5., -5., 5., 5.]],
            ..Default::default()
        };
        let frozen = shapes(&s, 1., false);
        assert!(!frozen.is_empty());
        assert_eq!(
            format!("{frozen:?}"),
            format!("{:?}", shapes(&s, 4., false))
        );
        assert_ne!(
            format!("{:?}", shapes(&s, 1., true)),
            format!("{:?}", shapes(&s, 4., true))
        );
        assert_eq!(s.fraction(), 0.25);
        assert!(
            shapes(&s, 1., true)
                .iter()
                .any(|s| matches!(s.shape, egui::Shape::LineSegment { .. }))
        );
        let back = Snapshot {
            bounds: vec![[175., -5., 185., 5.]],
            ..s.clone()
        };
        assert!(
            !shapes(&back, 1., true)
                .iter()
                .any(|s| matches!(s.shape, egui::Shape::LineSegment { .. }))
        );
        assert!(
            shapes(
                &Snapshot {
                    total: 4,
                    ready: 4,
                    fraction_sum: 4.,
                    ..Default::default()
                },
                1.,
                true
            )
            .is_empty()
        );
        let gpu = Snapshot {
            total: 4,
            ready: 4,
            fraction_sum: 4.,
            uploading: 1,
            ..Default::default()
        };
        assert!(gpu.active());
        assert!(gpu.fraction() < 1.);
        assert!(!shapes(&gpu, 1., false).is_empty());
        let failed = Snapshot {
            total: 2,
            failed: 2,
            ..Default::default()
        };
        assert!(failed.active());
        assert!(!failed.running());
    }
    #[test]
    fn globe_loading_gpu_versions_and_frame_reset_do_not_reuse_old_progress() {
        let ctx = egui::Context::default();
        begin(&ctx);
        watch_gpu(&ctx, 7, 10);
        assert_eq!(
            ctx.data(|d| d.get_temp::<Snapshot>(id()).unwrap().uploading),
            1
        );
        uploaded(&ctx, 7, 10);
        begin(&ctx);
        watch_gpu(&ctx, 7, 10);
        assert!(!ctx.data(|d| d.get_temp::<Snapshot>(id()).unwrap()).active());
        watch_gpu(&ctx, 7, 11);
        assert_eq!(
            ctx.data(|d| d.get_temp::<Snapshot>(id()).unwrap().uploading),
            1
        );
        pending_gpu(&ctx, 7);
        begin(&ctx);
        watch_gpu(&ctx, 7, 10);
        assert_eq!(
            ctx.data(|d| d.get_temp::<Snapshot>(id()).unwrap().uploading),
            1
        );
    }
    #[test]
    #[ignore = "renders the Globe loading HUD fixture on a GPU and saves a layout preview"]
    fn globe_loading_gpu_preview() {
        use super::super::contour_pass::tiles::gpu_tests::{gpu, pixels, target};
        use eframe::{egui_wgpu, wgpu};
        let (device, queue) = gpu();
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 720.));
        let view = GlobeViewState::from_focus(GeoPoint { lat: 0., lon: 0. });
        let layout = super::super::globe_scene::globe_layout(rect, &view);
        let snapshot = Snapshot {
            total: 12,
            ready: 3,
            fraction_sum: 5.5,
            building: 4,
            reading: 3,
            waiting: 2,
            preparing: 1,
            uploading: 2,
            bounds: vec![[-30., 5., -20., 15.], [5., -10., 15., 0.]],
            ..Default::default()
        };
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(rect),
                time: Some(1.5),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::background());
                painter.rect_filled(rect, 0., theme::canvas_background());
                painter.circle_stroke(
                    layout.center,
                    layout.radius,
                    egui::Stroke::new(1_f32, theme::panel_stroke()),
                );
                paint(&painter, rect, &layout, &view, &snapshot, 1.5, true);
            },
        );
        let jobs = ctx.tessellate(out.shapes, out.pixels_per_point);
        let mut renderer =
            egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, None, 1, false);
        for (id, delta) in out.textures_delta.set {
            renderer.update_texture(&device, &queue, id, &delta);
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [1024, 720],
            pixels_per_point: 1.,
        };
        let texture = target(&device, [1024, 720]);
        let mut encoder = device.create_command_encoder(&Default::default());
        let commands = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
        let view = texture.create_view(&Default::default());
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Globe HUD preview"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        queue.submit(
            commands
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        device.poll(wgpu::Maintain::Wait);
        let rgba = pixels(&device, &queue, &texture);
        let mut png = png::Encoder::new(
            std::fs::File::create("/tmp/1kee-global-loading-preview.png").unwrap(),
            1024,
            720,
        );
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header().unwrap().write_image_data(&rgba).unwrap();
        assert!(
            rgba.chunks_exact(4)
                .any(|p| p[0] > 60 || p[1] > 60 || p[2] > 60)
        );
    }
}
