//! High-DPI offscreen Cartographic Print / Poster Export.
//!
//! Renders the live GPU map scene into an isolated offscreen RGBA8 texture at
//! print resolution (150/300/600 DPI), paints a cartographic collar (neatline,
//! graticule ticks, haversine scale bar, north arrow, title/colophon), strips
//! 256-byte WGPU row alignment padding, and writes a PNG with a standard `pHYs`
//! DPI chunk (plus optional `.pgw` world file for GIS interop).

use crate::model::{ActiveBody, AppModel, GeoPoint, haversine_km};
use crate::theme;
use eframe::{egui_wgpu, wgpu};
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct PosterExportConfig {
    pub open: bool,
    pub title: String,
    pub subtitle: String,
    pub dpi: u32,
    pub width_px: u32,
    pub height_px: u32,
    pub include_collar: bool,
    pub write_world_file: bool,
    pub last_status: String,
}

impl Default for PosterExportConfig {
    fn default() -> Self {
        Self {
            open: false,
            title: "ONE THOUSAND ELECTRIC EYE".into(),
            subtitle: "Topographic Cartographic Plate".into(),
            dpi: 300,
            width_px: 2400,
            height_px: 1600,
            include_collar: true,
            write_world_file: true,
            last_status: String::new(),
        }
    }
}

/// Compute 256-byte aligned row pitch required by `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`.
pub fn aligned_bytes_per_row(width_px: u32) -> u32 {
    let unaligned = width_px.saturating_mul(4);
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unaligned.div_ceil(align) * align
}

/// Strip row-alignment padding from a mapped WGPU texture readback buffer.
pub fn strip_row_padding(padded: &[u8], width_px: u32, height_px: u32) -> Vec<u8> {
    let row_bytes = (width_px as usize) * 4;
    let stride = aligned_bytes_per_row(width_px) as usize;
    let mut tight = Vec::with_capacity(row_bytes * height_px as usize);
    for y in 0..height_px as usize {
        let start = y * stride;
        let end = (start + row_bytes).min(padded.len());
        if start < end {
            tight.extend_from_slice(&padded[start..end]);
        }
    }
    tight
}

/// Encode RGBA8 pixels to a PNG file with an explicit `pHYs` DPI chunk so print
/// apps open the poster at exact physical dimensions.
pub fn write_png_with_dpi(
    path: &Path,
    rgba: &[u8],
    width: u32,
    height: u32,
    dpi: u32,
) -> Result<(), String> {
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let ppm = ((dpi as f64) / 0.0254).round() as u32;
        encoder.set_pixel_dims(Some(png::PixelDimensions {
            xppu: ppm,
            yppu: ppm,
            unit: png::Unit::Meter,
        }));
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, encoded).map_err(|e| e.to_string())
}

/// Write an Esri `.pgw` world file next to the exported PNG for GIS georeferencing.
pub fn write_world_file(
    png_path: &Path,
    center: GeoPoint,
    half_extent_deg: f32,
    width_px: u32,
    height_px: u32,
) -> Result<(), String> {
    let pgw_path = png_path.with_extension("pgw");
    let aspect = (width_px as f64) / (height_px.max(1) as f64);
    let half_lat = half_extent_deg as f64;
    let half_lon = half_lat * aspect / center.lat.to_radians().cos().abs().max(0.15) as f64;
    let px_lon = (2.0 * half_lon) / width_px.max(1) as f64;
    let px_lat = -(2.0 * half_lat) / height_px.max(1) as f64;
    let west = center.lon as f64 - half_lon + px_lon * 0.5;
    let north = center.lat as f64 + half_lat + px_lat * 0.5;
    let body = format!("{px_lon:.10}\n0.0\n0.0\n{px_lat:.10}\n{west:.8}\n{north:.8}\n");
    std::fs::write(pgw_path, body).map_err(|e| e.to_string())
}

/// Paint a cartographic collar (neatline, graticule ticks, scale bar, north arrow,
/// title, and colophon) around `map_rect` inside the full `poster_rect`.
pub fn paint_cartographic_collar(
    painter: &egui::Painter,
    poster_rect: Rect,
    map_rect: Rect,
    model: &AppModel,
    config: &PosterExportConfig,
) {
    let bg = theme::canvas_background();
    let ink = if theme::is_atlas() {
        Color32::from_rgb(44, 39, 32)
    } else {
        Color32::from_rgb(225, 236, 240)
    };
    let muted = theme::text_muted();
    let accent = theme::topo_color();

    // Mask outer collar margins cleanly
    let top = Rect::from_min_max(poster_rect.min, Pos2::new(poster_rect.right(), map_rect.top()));
    let bot = Rect::from_min_max(Pos2::new(poster_rect.left(), map_rect.bottom()), poster_rect.max);
    let left = Rect::from_min_max(
        Pos2::new(poster_rect.left(), map_rect.top()),
        Pos2::new(map_rect.left(), map_rect.bottom()),
    );
    let right = Rect::from_min_max(
        Pos2::new(map_rect.right(), map_rect.top()),
        Pos2::new(poster_rect.right(), map_rect.bottom()),
    );
    for margin in [top, bot, left, right] {
        painter.rect_filled(margin, 0.0, bg);
    }

    // Double neatline frame
    painter.rect_stroke(map_rect, 0.0, Stroke::new(1.8, ink), egui::StrokeKind::Outside);
    painter.rect_stroke(
        map_rect.expand(5.0),
        0.0,
        Stroke::new(0.8, muted),
        egui::StrokeKind::Outside,
    );

    let center = if model.globe_view.local_mode {
        model.globe_view.local_center
    } else {
        model.globe_view.globe_center_latlon()
    };
    let half_deg = if model.globe_view.local_mode {
        super::vr_local_extent(model.globe_view.local_zoom)
    } else {
        (45.0 / model.globe_view.zoom.max(0.6)).clamp(0.5, 85.0)
    };
    let aspect = map_rect.width() / map_rect.height().max(1.0);
    let half_lon = half_deg * aspect / center.lat.to_radians().cos().abs().max(0.15);

    // Graticule ticks along top/bottom (lon) and left/right (lat)
    let tick_font = FontId::monospace(9.5);
    for i in 1..=5 {
        let frac = i as f32 / 6.0;
        let x = map_rect.left() + map_rect.width() * frac;
        let lon = center.lon - half_lon + 2.0 * half_lon * frac;
        painter.line_segment([Pos2::new(x, map_rect.top() - 5.0), Pos2::new(x, map_rect.top())], Stroke::new(0.9, ink));
        painter.line_segment([Pos2::new(x, map_rect.bottom()), Pos2::new(x, map_rect.bottom() + 5.0)], Stroke::new(0.9, ink));
        painter.text(
            Pos2::new(x, map_rect.top() - 8.0),
            Align2::CENTER_BOTTOM,
            format!("{lon:.2}°"),
            tick_font.clone(),
            muted,
        );

        let y = map_rect.top() + map_rect.height() * frac;
        let lat = center.lat + half_deg - 2.0 * half_deg * frac;
        painter.line_segment([Pos2::new(map_rect.left() - 5.0, y), Pos2::new(map_rect.left(), y)], Stroke::new(0.9, ink));
        painter.line_segment([Pos2::new(map_rect.right(), y), Pos2::new(map_rect.right() + 5.0, y)], Stroke::new(0.9, ink));
        painter.text(
            Pos2::new(map_rect.left() - 8.0, y),
            Align2::RIGHT_CENTER,
            format!("{lat:.2}°"),
            tick_font.clone(),
            muted,
        );
    }

    // Bottom cartographic title block
    let title_y = map_rect.bottom() + 18.0;
    painter.text(
        Pos2::new(map_rect.left(), title_y),
        Align2::LEFT_TOP,
        &config.title,
        FontId::proportional(17.0),
        ink,
    );
    let body_name = match model.active_body {
        ActiveBody::Earth => "EARTH · WGS84",
        ActiveBody::Moon => "MOON · SLDEM2015",
        ActiveBody::Mars => "MARS · MOLA/CTX",
    };
    let meta = format!(
        "{}  |  {}  |  {:.4}°, {:.4}°  |  Theme: {}  |  {} DPI",
        config.subtitle,
        body_name,
        center.lat,
        center.lon,
        model.map_theme.label(),
        config.dpi,
    );
    painter.text(
        Pos2::new(map_rect.left(), title_y + 22.0),
        Align2::LEFT_TOP,
        meta,
        FontId::monospace(10.0),
        muted,
    );

    // Dynamic haversine scale bar on bottom-right
    let span_km = haversine_km(
        GeoPoint { lat: center.lat, lon: center.lon - half_lon },
        GeoPoint { lat: center.lat, lon: center.lon + half_lon },
    )
    .max(0.5);
    let target_km = nice_scale_km(span_km * 0.18);
    let bar_px = ((target_km / span_km) * map_rect.width()).clamp(40.0, 200.0);
    let bar_right = map_rect.right() - 48.0;
    let bar_left = bar_right - bar_px;
    let bar_y = title_y + 14.0;
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(bar_left, bar_y), Pos2::new(bar_left + bar_px * 0.5, bar_y + 5.0)),
        0.0,
        ink,
    );
    painter.rect_stroke(
        Rect::from_min_max(Pos2::new(bar_left, bar_y), Pos2::new(bar_right, bar_y + 5.0)),
        0.0,
        Stroke::new(1.0, ink),
        egui::StrokeKind::Inside,
    );
    painter.text(
        Pos2::new((bar_left + bar_right) * 0.5, bar_y + 9.0),
        Align2::CENTER_TOP,
        format!("{target_km:.0} km"),
        FontId::monospace(10.0),
        ink,
    );

    // Crisp North arrow to the right of the scale bar
    let na = Pos2::new(map_rect.right() - 18.0, title_y + 22.0);
    let tip = na + Vec2::new(0.0, -14.0);
    let left_pt = na + Vec2::new(-5.5, 4.0);
    let right_pt = na + Vec2::new(5.5, 4.0);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, na, left_pt],
        accent,
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![tip, right_pt, na],
        ink,
        Stroke::NONE,
    ));
    painter.text(
        tip + Vec2::new(0.0, -3.0),
        Align2::CENTER_BOTTOM,
        "N",
        FontId::monospace(10.0),
        ink,
    );
}

fn nice_scale_km(raw: f32) -> f32 {
    const STEPS: &[f32] = &[
        1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 5000.0,
    ];
    STEPS
        .iter()
        .copied()
        .find(|&s| s >= raw)
        .unwrap_or(5000.0)
}

/// Render the current map view offscreen at high DPI on the live WGPU device and save to disk.
pub fn render_and_save(
    render_state: &egui_wgpu::RenderState,
    model: &mut AppModel,
    config: &PosterExportConfig,
    path: &Path,
) -> Result<(), String> {
    let width = config.width_px.clamp(640, 6144);
    let height = config.height_px.clamp(480, 4096);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let device = &render_state.device;
    let queue = &render_state.queue;

    let mut offscreen_renderer = egui_wgpu::Renderer::new(device, format, None, 1, false);
    crate::app::DashboardApp::register_passes(device, format, &mut offscreen_renderer);

    let ctx = egui::Context::default();
    theme::set_theme(&ctx, model.map_theme);

    let ppp = (config.dpi as f32 / 96.0).clamp(1.0, 4.0);
    let logical_w = width as f32 / ppp;
    let logical_h = height as f32 / ppp;
    let full_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(logical_w, logical_h));
    let map_rect = if config.include_collar {
        Rect::from_min_max(
            Pos2::new(56.0, 34.0),
            Pos2::new((logical_w - 36.0).max(120.0), (logical_h - 68.0).max(120.0)),
        )
    } else {
        full_rect
    };

    let saved_obs = model.obs_view;
    model.obs_view = true;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(full_rect),
            ..Default::default()
        },
        |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            painter.rect_filled(full_rect, 0.0, theme::canvas_background());
            let map_painter = painter.with_clip_rect(map_rect);
            if model.globe_view.local_mode {
                super::local_terrain_scene::paint(&map_painter, map_rect, model, 0.0);
            } else {
                super::globe_scene::paint(&map_painter, map_rect, model, 0.0);
            }
            if config.include_collar {
                paint_cartographic_collar(&painter, full_rect, map_rect, model, config);
            }
        },
    );
    model.obs_view = saved_obs;

    let paint_jobs = ctx.tessellate(output.shapes, ppp);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [width, height],
        pixels_per_point: ppp,
    };

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("poster_export_target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    for (id, delta) in &output.textures_delta.set {
        offscreen_renderer.update_texture(device, queue, *id, delta);
    }

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("poster_export_encoder"),
    });
    let user_cmds =
        offscreen_renderer.update_buffers(device, queue, &mut encoder, &paint_jobs, &screen);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("poster_export_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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
        let mut pass = pass.forget_lifetime();
        offscreen_renderer.render(&mut pass, &paint_jobs, &screen);
    }

    let stride = aligned_bytes_per_row(width);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("poster_export_readback"),
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(user_cmds.into_iter().chain(std::iter::once(encoder.finish())));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |res| {
        let _ = tx.send(res);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let padded = slice.get_mapped_range();
    let tight = strip_row_padding(&padded, width, height);
    drop(padded);
    readback.unmap();

    write_png_with_dpi(path, &tight, width, height, config.dpi)?;
    if config.write_world_file {
        let center = if model.globe_view.local_mode {
            model.globe_view.local_center
        } else {
            model.globe_view.globe_center_latlon()
        };
        let half = if model.globe_view.local_mode {
            super::vr_local_extent(model.globe_view.local_zoom)
        } else {
            (45.0 / model.globe_view.zoom.max(0.6)).clamp(0.5, 85.0)
        };
        write_world_file(path, center, half, width, height)?;
    }
    Ok(())
}

/// Floating Poster Export dialog rendered when `model.poster_export.open` is true.
pub fn render_dialog(
    ctx: &egui::Context,
    model: &mut AppModel,
    render_state: Option<&egui_wgpu::RenderState>,
) {
    if !model.poster_export.open {
        return;
    }
    let mut open = model.poster_export.open;
    let mut trigger_export = false;
    egui::Window::new("Export Cartographic Poster")
        .open(&mut open)
        .resizable(false)
        .default_width(340.0)
        .show(ctx, |ui| {
            ui.label("Title");
            ui.text_edit_singleline(&mut model.poster_export.title);
            ui.label("Subtitle");
            ui.text_edit_singleline(&mut model.poster_export.subtitle);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("DPI");
                ui.selectable_value(&mut model.poster_export.dpi, 150, "150");
                ui.selectable_value(&mut model.poster_export.dpi, 300, "300");
                ui.selectable_value(&mut model.poster_export.dpi, 600, "600");
            });
            ui.horizontal(|ui| {
                ui.label("Resolution");
                if ui.small_button("2400×1600").clicked() {
                    model.poster_export.width_px = 2400;
                    model.poster_export.height_px = 1600;
                }
                if ui.small_button("3600×2400").clicked() {
                    model.poster_export.width_px = 3600;
                    model.poster_export.height_px = 2400;
                }
                if ui.small_button("4800×3200").clicked() {
                    model.poster_export.width_px = 4800;
                    model.poster_export.height_px = 3200;
                }
            });
            ui.checkbox(
                &mut model.poster_export.include_collar,
                "Include neatline, graticule ticks, scale bar & north arrow",
            );
            ui.checkbox(
                &mut model.poster_export.write_world_file,
                "Write GIS world file (.pgw sidecar)",
            );
            ui.add_space(6.0);
            ui.add_enabled_ui(render_state.is_some(), |ui| {
                if ui.button("Save High-DPI Poster PNG…").clicked() {
                    trigger_export = true;
                }
            });
            if !model.poster_export.last_status.is_empty() {
                ui.small(&model.poster_export.last_status);
            }
        });
    model.poster_export.open = open;

    if trigger_export
        && let Some(state) = render_state
        && let Some(path) = rfd::FileDialog::new()
            .set_file_name("1kee_poster.png")
            .add_filter("PNG Poster", &["png"])
            .save_file()
    {
        let cfg = model.poster_export.clone();
        match render_and_save(state, model, &cfg, &path) {
            Ok(()) => {
                model.poster_export.last_status = format!("Saved {}", path.display());
                model.push_log(format!("Exported high-DPI poster to {}", path.display()));
            }
            Err(err) => {
                model.poster_export.last_status = format!("Export failed: {err}");
                model.push_log(format!("Poster export failed: {err}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_alignment_and_padding_strip_handle_odd_widths() {
        assert_eq!(aligned_bytes_per_row(100), 512);
        assert_eq!(aligned_bytes_per_row(64), 256);
        let width = 3;
        let height = 2;
        let stride = aligned_bytes_per_row(width) as usize;
        let mut padded = vec![0u8; stride * height];
        for i in 0..12 {
            padded[i] = (i + 1) as u8;
            padded[stride + i] = (i + 21) as u8;
        }
        let tight = strip_row_padding(&padded, width as u32, height as u32);
        assert_eq!(tight.len(), 24);
        assert_eq!(tight[0], 1);
        assert_eq!(tight[11], 12);
        assert_eq!(tight[12], 21);
        assert_eq!(tight[23], 32);
    }

    #[test]
    fn png_with_dpi_writes_phys_chunk_and_world_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plate.png");
        let rgba = vec![255u8; 4 * 4 * 4];
        write_png_with_dpi(&path, &rgba, 4, 4, 300).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(4).any(|w| w == b"pHYs"));
        write_world_file(&path, GeoPoint { lat: 37.77, lon: -122.42 }, 0.5, 2400, 1600).unwrap();
        let pgw = std::fs::read_to_string(path.with_extension("pgw")).unwrap();
        assert_eq!(pgw.lines().count(), 6);
    }
}
