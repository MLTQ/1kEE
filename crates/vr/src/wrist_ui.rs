//! Direct-GPU Half-Life: Alyx–style forearm HUD rendered straight onto the
//! OpenXR `wgpu::Device` with zero CPU readback and 90/120 Hz responsiveness.
use crate::wrist::{HUD_DPI, HUD_LOGICAL, HUD_SIZE};
use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke, Vec2};
use one_thousand_electric_eye_desktop::vr_operator::{
    MAX_RANGE_M, MIN_RANGE_M, VrCommand, VrLayer, VrSnapshot,
};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Map,
    Layers,
    Events,
    Brief,
    Cams,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct WristFeedback {
    pub hover_tick: bool,
    pub click_tick: bool,
    pub toggle_pin: bool,
}

pub struct WristHud {
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    tab: Tab,
    start: Instant,
    last_hovered_id: Option<egui::Id>,
}

impl WristHud {
    pub fn new(device: &wgpu::Device) -> Self {
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vr wrist hud direct target"),
            size: wgpu::Extent3d {
                width: HUD_SIZE[0],
                height: HUD_SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let renderer = egui_wgpu::Renderer::new(device, format, None, 1, false);
        let ctx = egui::Context::default();
        ctx.options_mut(|o| {
            o.input_options.max_click_dist = 14.0;
            o.input_options.max_click_duration = 1.5;
        });
        Self::apply_style(&ctx);
        Self {
            ctx,
            renderer,
            _texture: texture,
            view,
            tab: Tab::Map,
            start: Instant::now(),
            last_hovered_id: None,
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    fn apply_style(ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        let mut v = egui::Visuals::dark();
        v.panel_fill = Color32::from_rgba_unmultiplied(10, 18, 24, 232);
        v.window_fill = Color32::from_rgba_unmultiplied(12, 22, 29, 238);
        v.override_text_color = Some(Color32::from_rgb(228, 242, 244));
        v.widgets.inactive.bg_fill = Color32::from_rgb(22, 37, 47);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(22, 37, 47);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(48, 80, 96));
        v.widgets.hovered.bg_fill = Color32::from_rgb(31, 56, 68);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(31, 56, 68);
        v.widgets.hovered.bg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(104, 238, 206));
        v.widgets.active.bg_fill = Color32::from_rgb(38, 96, 86);
        v.widgets.active.bg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(156, 255, 228));
        v.selection.bg_fill = Color32::from_rgb(34, 108, 96);
        v.selection.stroke = Stroke::new(1.2_f32, Color32::from_rgb(122, 245, 212));
        style.visuals = v;
        style.spacing.item_spacing = Vec2::new(4.0, 4.0);
        style.spacing.button_padding = Vec2::new(5.0, 3.0);
        style.spacing.interact_size = Vec2::new(26.0, 22.0);
        style.spacing.slider_width = 165.0;
        for (ts, sz) in [
            (egui::TextStyle::Small, 9.0),
            (egui::TextStyle::Body, 10.5),
            (egui::TextStyle::Button, 10.5),
            (egui::TextStyle::Heading, 12.5),
            (egui::TextStyle::Monospace, 10.0),
        ] {
            style.text_styles.insert(ts, egui::FontId::proportional(sz));
        }
        ctx.set_style(style);
    }

    /// Run one frame of the direct-GPU wrist HUD and record its render pass
    /// straight onto the main OpenXR command encoder.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        events: Vec<egui::Event>,
        snapshot: &mut VrSnapshot,
        pinned: bool,
    ) -> (Vec<VrCommand>, WristFeedback) {
        let mut commands = Vec::new();
        let mut feedback = WristFeedback::default();

        let rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(HUD_LOGICAL[0], HUD_LOGICAL[1]),
        );
        let mut raw = egui::RawInput {
            screen_rect: Some(rect),
            time: Some(self.start.elapsed().as_secs_f64()),
            max_texture_side: Some(2048),
            events,
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(HUD_DPI);

        let clicked_before = raw
            .events
            .iter()
            .any(|e| matches!(e, egui::Event::PointerButton { pressed: true, .. }));

        let tab = &mut self.tab;
        let output = self.ctx.run(raw, |ctx| {
            Self::build_ui(tab, ctx, snapshot, pinned, &mut commands, &mut feedback);
        });

        let hovered_now = self.ctx.input(|i| i.pointer.hover_pos()).and_then(|_| {
            // Detect entering a new interactive widget for subtle haptic ticks.
            self.ctx
                .memory(|m| m.focused().or_else(|| self.ctx.layer_id_at(egui::Pos2::ZERO).map(|l| l.id)))
        });
        let any_hovered = self.ctx.is_pointer_over_area();
        if clicked_before && !commands.is_empty() {
            feedback.click_tick = true;
        }
        if any_hovered && hovered_now != self.last_hovered_id {
            feedback.hover_tick = true;
        }
        self.last_hovered_id = hovered_now;

        for (id, delta) in &output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        let jobs = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: HUD_SIZE,
            pixels_per_point: output.pixels_per_point,
        };
        let extra = self
            .renderer
            .update_buffers(device, queue, encoder, &jobs, &screen);
        if !extra.is_empty() {
            queue.submit(extra);
        }
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("vr wrist hud direct pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }

        (commands, feedback)
    }

    fn build_ui(
        tab: &mut Tab,
        ctx: &egui::Context,
        snap: &mut VrSnapshot,
        pinned: bool,
        cmds: &mut Vec<VrCommand>,
        fb: &mut WristFeedback,
    ) {
        let header_bg = Color32::from_rgba_unmultiplied(12, 24, 32, 245);
        let accent = Color32::from_rgb(104, 238, 206);
        let amber = Color32::from_rgb(244, 174, 68);
        let muted = Color32::from_rgb(138, 172, 184);

        // ── Telemetry Header (HL:Alyx gauntlet readout) ──
        egui::TopBottomPanel::top("alyx-header")
            .exact_height(24.0)
            .frame(
                Frame::new()
                    .fill(header_bg)
                    .inner_margin(Margin::symmetric(8, 3)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(snap.view.scale_label())
                            .strong()
                            .size(11.0)
                            .color(accent),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{:.2}°, {:.2}°",
                            snap.view.lat, snap.view.lon
                        ))
                        .size(9.5)
                        .color(muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let pin_btn = egui::Button::new(
                            RichText::new(if pinned { "PINNED" } else { "PIN" })
                                .size(9.0)
                                .strong()
                                .color(if pinned { Color32::BLACK } else { amber }),
                        )
                        .fill(if pinned {
                            amber
                        } else {
                            Color32::from_rgb(36, 32, 20)
                        })
                        .stroke(Stroke::new(1.0_f32, amber))
                        .corner_radius(CornerRadius::same(4));
                        if ui.add_sized([48.0, 17.0], pin_btn).clicked() {
                            fb.toggle_pin = true;
                            fb.click_tick = true;
                        }
                    });
                });
            });

        // ── 5-Tab Navigation Bar ──
        egui::TopBottomPanel::top("alyx-tabs")
            .exact_height(26.0)
            .frame(
                Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(14, 28, 36, 240))
                    .inner_margin(Margin::symmetric(6, 3)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 12.0) / 5.0;
                    for (candidate, label) in [
                        (Tab::Map, "MAP"),
                        (Tab::Layers, "LAYERS"),
                        (Tab::Events, "EVENTS"),
                        (Tab::Brief, "BRIEF"),
                        (Tab::Cams, "CAMS"),
                    ] {
                        let active = *tab == candidate;
                        let btn = egui::Button::new(
                            RichText::new(label)
                                .size(9.5)
                                .strong()
                                .color(if active { Color32::BLACK } else { Color32::WHITE }),
                        )
                        .fill(if active {
                            accent
                        } else {
                            Color32::from_rgb(24, 42, 52)
                        })
                        .corner_radius(CornerRadius::same(4));
                        if ui.add_sized([w, 20.0], btn).clicked() {
                            *tab = candidate;
                            fb.click_tick = true;
                        }
                    }
                });
            });

        // ── Main Content Panel ──
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(9, 17, 23, 224))
                    .inner_margin(Margin::symmetric(8, 6)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("alyx-scroll", *tab as u8))
                    .auto_shrink([false, false])
                    .drag_to_scroll(false)
                    .show(ui, |ui| match *tab {
                        Tab::Map => Self::tab_map(ui, snap, cmds, accent, amber, muted),
                        Tab::Layers => Self::tab_layers(ui, snap, cmds, accent, amber, muted),
                        Tab::Events => Self::tab_events(tab, ui, snap, cmds, accent, muted),
                        Tab::Brief => Self::tab_brief(ui, snap, cmds, accent, muted),
                        Tab::Cams => Self::tab_cams(ui, snap, cmds, accent, muted),
                    });
            });
    }

    fn tab_map(
        ui: &mut egui::Ui,
        snap: &mut VrSnapshot,
        cmds: &mut Vec<VrCommand>,
        accent: Color32,
        amber: Color32,
        muted: Color32,
    ) {
        // 1-tap continuous scale presets (Planet → Street)
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 9.0) / 4.0;
            for (label, target_m) in [
                ("ORBIT", 18_000_000.0),
                ("REGION", 600_000.0),
                ("CITY", 25_000.0),
                ("STREET", 400.0),
            ] {
                let near = (snap.view.range_m / target_m).ln().abs() < 0.7;
                if pill_button(ui, [w, 24.0], label, near, accent).clicked() {
                    snap.view = one_thousand_electric_eye_desktop::vr_operator::View::from_continuous(
                        snap.view.lat,
                        snap.view.lon,
                        target_m,
                    );
                    cmds.push(VrCommand::SetRange(target_m));
                }
            }
        });

        ui.add_space(3.0);
        let mut range = snap.view.range_m.clamp(MIN_RANGE_M, MAX_RANGE_M);
        let resp = ui.add(
            egui::Slider::new(&mut range, MIN_RANGE_M..=MAX_RANGE_M)
                .logarithmic(true)
                .show_value(false)
                .text(format!("Scale {}", snap.view.scale_label())),
        );
        if resp.changed() {
            snap.view = one_thousand_electric_eye_desktop::vr_operator::View::from_continuous(
                snap.view.lat,
                snap.view.lon,
                range,
            );
            cmds.push(VrCommand::SetRange(range));
        }

        ui.add_space(4.0);
        // Projection, Theme, Follow row
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 6.0) / 3.0;
            let proj_lbl = if snap.layers.projection_ortho {
                "ORTHO"
            } else {
                "PERSP"
            };
            if pill_button(ui, [w, 24.0], proj_lbl, snap.layers.projection_ortho, accent).clicked()
            {
                snap.layers.projection_ortho = !snap.layers.projection_ortho;
                cmds.push(VrCommand::ToggleProjection);
            }
            if pill_button(
                ui,
                [w, 24.0],
                &snap.layers.theme_name.to_uppercase(),
                false,
                accent,
            )
            .clicked()
            {
                cmds.push(VrCommand::CycleTheme);
            }
            if pill_button(ui, [w, 24.0], "FOLLOW", snap.layers.follow_events, amber).clicked() {
                snap.layers.follow_events = !snap.layers.follow_events;
                cmds.push(VrCommand::ToggleFollow);
            }
        });

        ui.add_space(3.0);
        // Action row: Recenter, Room HUD, Replay, Exit VR
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 9.0) / 4.0;
            if pill_button(ui, [w, 24.0], "CENTER", false, accent).clicked() {
                cmds.push(VrCommand::Recenter);
            }
            if pill_button(ui, [w, 24.0], "HOLOPAD", snap.layers.room_visible, accent).clicked() {
                snap.layers.room_visible = !snap.layers.room_visible;
                cmds.push(VrCommand::ToggleRoom);
            }
            if pill_button(ui, [w, 24.0], "REPLAY", snap.layers.replay_mode, amber).clicked() {
                snap.layers.replay_mode = !snap.layers.replay_mode;
                cmds.push(VrCommand::ToggleReplay);
            }
            let exit_btn = egui::Button::new(
                RichText::new("EXIT")
                    .size(9.5)
                    .strong()
                    .color(Color32::from_rgb(255, 130, 118)),
            )
            .fill(Color32::from_rgb(56, 20, 22))
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(175, 64, 58)))
            .corner_radius(CornerRadius::same(4));
            if ui.add_sized([w, 24.0], exit_btn).clicked() {
                cmds.push(VrCommand::Exit);
            }
        });

        ui.add_space(5.0);
        ui.label(
            RichText::new("Grip: Grab/Pan  ·  2-Hand Grip: Pinch Zoom  ·  Stick: Orbit")
                .size(8.5)
                .color(muted),
        );
    }

    fn tab_layers(
        ui: &mut egui::Ui,
        snap: &mut VrSnapshot,
        cmds: &mut Vec<VrCommand>,
        accent: Color32,
        amber: Color32,
        muted: Color32,
    ) {
        // Featured Fused Highest-Density LiDAR Point Cloud Layer (default 1 px)
        let pc_on = snap.layers.point_cloud;
        Frame::new()
            .fill(if pc_on {
                Color32::from_rgb(20, 48, 44)
            } else {
                Color32::from_rgb(18, 30, 38)
            })
            .stroke(Stroke::new(
                1.0_f32,
                if pc_on {
                    accent
                } else {
                    Color32::from_rgb(48, 76, 90)
                },
            ))
            .corner_radius(CornerRadius::same(5))
            .inner_margin(Margin::symmetric(6, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if pill_button(ui, [156.0, 22.0], "LIDAR CLOUD (FUSED)", pc_on, accent)
                        .clicked()
                    {
                        snap.layers.point_cloud = !snap.layers.point_cloud;
                        cmds.push(VrCommand::ToggleLayer(VrLayer::PointCloud));
                    }
                    ui.label(
                        RichText::new(&snap.layers.point_cloud_status)
                            .size(8.5)
                            .color(if pc_on { amber } else { muted }),
                    );
                });
                if pc_on {
                    let mut px = snap.layers.point_cloud_size_px;
                    if ui
                        .add(
                            egui::Slider::new(&mut px, 1.0..=8.0)
                                .step_by(0.5)
                                .text("Point px"),
                        )
                        .changed()
                    {
                        snap.layers.point_cloud_size_px = px;
                        cmds.push(VrCommand::SetPointCloudSize(px));
                    }
                }
            });

        ui.add_space(4.0);
        // 2-column tactical layer grid
        let col_w = (ui.available_width() - 4.0) * 0.5;
        let items: [(VrLayer, &str, bool); 13] = [
            (VrLayer::Contours, "Contours", snap.layers.contours),
            (VrLayer::Coastlines, "Coastlines", snap.layers.coastlines),
            (VrLayer::Bathymetry, "Bathymetry", snap.layers.bathymetry),
            (VrLayer::Roads, "Roads", snap.layers.roads),
            (VrLayer::Buildings, "Buildings", snap.layers.buildings),
            (VrLayer::Water, "Water", snap.layers.water),
            (VrLayer::Trees, "Forest", snap.layers.trees),
            (VrLayer::Weather, "Weather IR", snap.layers.weather),
            (VrLayer::Events, "Events", snap.layers.events),
            (VrLayer::Cameras, "Webcams", snap.layers.cameras),
            (VrLayer::Ships, "Ships", snap.layers.ships),
            (VrLayer::Flights, "Flights", snap.layers.flights),
            (VrLayer::Seismic, "Seismic", snap.layers.seismic),
        ];
        for pair in items.chunks(2) {
            ui.horizontal(|ui| {
                for &(layer, name, active) in pair {
                    if pill_button(ui, [col_w, 22.0], name, active, accent).clicked() {
                        toggle_local_layer(&mut snap.layers, layer);
                        cmds.push(VrCommand::ToggleLayer(layer));
                    }
                }
            });
        }

        ui.add_space(3.0);
        let mut spread = snap.layers.relief_spread;
        if ui
            .add(egui::Slider::new(&mut spread, 0.0..=3.0).text("3D Relief"))
            .changed()
        {
            snap.layers.relief_spread = spread;
            cmds.push(VrCommand::SetReliefSpread(spread));
        }
    }

    fn tab_events(
        tab: &mut Tab,
        ui: &mut egui::Ui,
        snap: &mut VrSnapshot,
        cmds: &mut Vec<VrCommand>,
        accent: Color32,
        muted: Color32,
    ) {
        ui.horizontal(|ui| {
            if pill_button(ui, [110.0, 22.0], "FOLLOW EVENTS", snap.layers.follow_events, accent)
                .clicked()
            {
                snap.layers.follow_events = !snap.layers.follow_events;
                cmds.push(VrCommand::ToggleFollow);
            }
            ui.label(
                RichText::new(format!("{} live", snap.events.len()))
                    .size(9.0)
                    .color(muted),
            );
        });
        ui.add_space(3.0);
        if snap.events.is_empty() {
            ui.label(RichText::new(&snap.event_status).size(9.5).color(muted));
            return;
        }
        let w = ui.available_width();
        for ev in snap.events.iter().take(16) {
            let col = Color32::from_rgb(ev.severity_rgb[0], ev.severity_rgb[1], ev.severity_rgb[2]);
            let btn = egui::Button::new(
                RichText::new(format!("{} · {}\n{}", ev.severity_label, ev.location, ev.title))
                    .size(9.0)
                    .color(Color32::WHITE),
            )
            .fill(Color32::from_rgb(19, 32, 41))
            .stroke(Stroke::new(1.0_f32, col.gamma_multiply(0.75)))
            .truncate()
            .corner_radius(CornerRadius::same(4));
            if ui.add_sized([w, 30.0], btn).clicked() {
                cmds.push(VrCommand::SelectEvent(ev.id.clone()));
                *tab = Tab::Brief;
            }
        }
    }

    fn tab_brief(
        ui: &mut egui::Ui,
        snap: &mut VrSnapshot,
        cmds: &mut Vec<VrCommand>,
        accent: Color32,
        muted: Color32,
    ) {
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 4.0) * 0.5;
            if pill_button(ui, [w, 22.0], "HOLOPAD BRIEF", snap.layers.room_visible, accent)
                .clicked()
            {
                snap.layers.room_visible = !snap.layers.room_visible;
                cmds.push(VrCommand::ToggleRoom);
            }
            if pill_button(ui, [w, 22.0], "FOCUS MAP", false, accent).clicked() {
                cmds.push(VrCommand::FocusSelectedEvent);
            }
        });
        ui.add_space(4.0);
        if let Some(ev) = &snap.selected_event {
            let col = Color32::from_rgb(ev.severity_rgb[0], ev.severity_rgb[1], ev.severity_rgb[2]);
            ui.label(RichText::new(&ev.title).strong().size(11.0));
            ui.label(
                RichText::new(format!(
                    "{} · {} · {}",
                    ev.severity_label, ev.location, ev.source
                ))
                .size(9.0)
                .color(col),
            );
            ui.add_space(2.0);
            ui.label(RichText::new(&ev.summary).size(9.5));
        } else {
            ui.label(
                RichText::new("Point at any glowing spire on the map or pick an event in EVENTS.")
                    .size(9.5)
                    .color(muted),
            );
        }
    }

    fn tab_cams(
        ui: &mut egui::Ui,
        snap: &mut VrSnapshot,
        cmds: &mut Vec<VrCommand>,
        accent: Color32,
        muted: Color32,
    ) {
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 6.0) / 3.0;
            if pill_button(ui, [w, 22.0], "HOLOPAD", snap.layers.room_visible, accent).clicked() {
                snap.layers.room_visible = !snap.layers.room_visible;
                cmds.push(VrCommand::ToggleRoom);
            }
            if pill_button(ui, [w, 22.0], "AUTO NEAR", snap.layers.auto_camera, accent).clicked() {
                snap.layers.auto_camera = !snap.layers.auto_camera;
                cmds.push(VrCommand::ToggleAutoCamera);
            }
            if pill_button(ui, [w, 22.0], "STOP", false, accent).clicked() {
                cmds.push(VrCommand::StopCamera);
            }
        });
        ui.add_space(3.0);
        if let Some(lbl) = &snap.playing_camera_label {
            ui.label(RichText::new(format!("LIVE: {lbl}")).size(9.5).color(accent));
        } else {
            ui.label(RichText::new(&snap.camera_status).size(9.0).color(muted));
        }
        ui.add_space(3.0);
        let w = ui.available_width();
        for cam in snap.cameras.iter().take(12) {
            let active = snap.playing_camera_id.as_deref() == Some(cam.id.as_str());
            if pill_button(
                ui,
                [w, 22.0],
                &format!("{} · {:.0} km", cam.label, cam.distance_km),
                active,
                accent,
            )
            .clicked()
            {
                cmds.push(VrCommand::SelectCamera(cam.id.clone()));
            }
        }
    }
}

fn pill_button(
    ui: &mut egui::Ui,
    size: [f32; 2],
    label: &str,
    active: bool,
    accent: Color32,
) -> egui::Response {
    let btn = egui::Button::new(
        RichText::new(label)
            .size(9.5)
            .strong()
            .color(if active {
                Color32::from_rgb(10, 22, 24)
            } else {
                Color32::from_rgb(220, 238, 242)
            }),
    )
    .fill(if active {
        accent
    } else {
        Color32::from_rgb(22, 38, 48)
    })
    .stroke(Stroke::new(
        1.0_f32,
        if active {
            Color32::from_rgb(170, 255, 232)
        } else {
            Color32::from_rgb(48, 78, 94)
        },
    ))
    .truncate()
    .corner_radius(CornerRadius::same(4));
    ui.add_sized(size, btn)
}

fn toggle_local_layer(s: &mut one_thousand_electric_eye_desktop::vr_operator::VrLayerState, l: VrLayer) {
    match l {
        VrLayer::PointCloud => s.point_cloud = !s.point_cloud,
        VrLayer::Contours => s.contours = !s.contours,
        VrLayer::Coastlines => s.coastlines = !s.coastlines,
        VrLayer::Bathymetry => s.bathymetry = !s.bathymetry,
        VrLayer::Roads => s.roads = !s.roads,
        VrLayer::Buildings => s.buildings = !s.buildings,
        VrLayer::Water => s.water = !s.water,
        VrLayer::Trees => s.trees = !s.trees,
        VrLayer::Weather => s.weather = !s.weather,
        VrLayer::Events => s.events = !s.events,
        VrLayer::Cameras => s.cameras = !s.cameras,
        VrLayer::Ships => s.ships = !s.ships,
        VrLayer::Flights => s.flights = !s.flights,
        VrLayer::Seismic => s.seismic = !s.seismic,
    }
}
