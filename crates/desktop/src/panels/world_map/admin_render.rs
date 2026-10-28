//! Local administrative polygons, clipped without inventing border connections.
use super::{
    admin_layer,
    local_terrain_scene::{LocalLayout, projection, visual_half_extent_for_zoom},
};
use crate::model::{ActiveBody, AppModel, GeoPoint};
use egui::{Color32, Pos2, Rect, Shape, Stroke};

pub(super) fn draw(
    painter: &egui::Painter,
    layout: &LocalLayout,
    model: &AppModel,
    center: GeoPoint,
) {
    if !model.show_admin || model.active_body != ActiveBody::Earth {
        return;
    }
    painter
        .ctx()
        .request_repaint_after(std::time::Duration::from_secs(3));
    let Some(data) = admin_layer::get_for(model) else {
        return;
    };
    let half = visual_half_extent_for_zoom(model.globe_view.local_zoom);
    let params = projection::local_projection_params(
        layout,
        &model.globe_view,
        center,
        (half * 111.32 * center.lat.to_radians().cos().abs().max(0.2)).max(1.0),
        (half * 111.32).max(1.0),
    );
    paint(
        painter,
        &data,
        model.admin_options.shade,
        model.admin_options.opacity,
        center.lon,
        |point| {
            let (x, y, _) = params.project_logical(point, 0.0);
            egui::pos2(x, y)
        },
    );
}

fn fill_color(id: i64, level: u8, opacity: f32) -> Color32 {
    let base = crate::theme::admin_color(level);
    // Stable per-region variation helps distinguish adjacent regions.
    let mix = (id as u64).wrapping_mul(0x9e3779b97f4a7c15);
    let weight = 0.65 + ((mix >> 56) as f32 / 255.0) * 0.35;
    Color32::from_rgba_unmultiplied(
        (base.r() as f32 * weight) as u8,
        (base.g() as f32 * weight) as u8,
        (base.b() as f32 * weight) as u8,
        (opacity.clamp(0.0, 0.5) * 255.0) as u8,
    )
}

pub(super) fn paint(
    painter: &egui::Painter,
    data: &admin_layer::AdminData,
    shade: bool,
    opacity: f32,
    focus_lon: f32,
    project: impl Fn(GeoPoint) -> Pos2,
) {
    let clip = painter.clip_rect();
    let visible: Vec<_> = data
        .regions
        .iter()
        .filter_map(|region| {
            let [x0, y0, x1, y1] = region.bounds;
            let shift = ((focus_lon - (x0 + x1) * 0.5) / 360.0).round() * 360.0;
            let corners = [(x0, y0), (x0, y1), (x1, y0), (x1, y1)].map(|(lon, lat)| {
                project(GeoPoint {
                    lon: lon + shift,
                    lat,
                })
            });
            let bounds = Rect::from_points(&corners);
            (bounds.is_finite() && bounds.intersects(clip)).then_some((region, shift))
        })
        .collect();
    // Fill every region before any outline so shading cannot obscure borders.
    if shade {
        for &(region, shift) in &visible {
            let Some(fill) = &region.fill else {
                continue;
            };
            let vertices: Vec<_> = fill
                .vertices
                .iter()
                .map(|p| {
                    project(GeoPoint {
                        lon: p.lon + shift,
                        lat: p.lat,
                    })
                })
                .collect();
            let color = fill_color(region.relation_id, region.level, opacity);
            let mut mesh = egui::Mesh::default();
            for indices in fill.indices.chunks_exact(3) {
                let triangle = indices
                    .iter()
                    .map(|&i| vertices[i as usize])
                    .collect::<Vec<_>>();
                if !Rect::from_points(&triangle).intersects(clip) {
                    continue;
                }
                let polygon = clip_polygon(triangle, clip);
                let start = mesh.vertices.len() as u32;
                for p in &polygon {
                    mesh.colored_vertex(*p, color);
                }
                for i in 1..polygon.len().saturating_sub(1) {
                    mesh.add_triangle(start, start + i as u32, start + i as u32 + 1);
                }
            }
            if !mesh.is_empty() {
                painter.add(Shape::mesh(mesh));
            }
        }
    }
    for level in (2..=11).rev() {
        let stroke = Stroke::new(
            crate::theme::admin_stroke_width(level).max(0.8),
            crate::theme::admin_color(level),
        );
        for &(region, shift) in &visible {
            if region.level != level {
                continue;
            }
            for ring in &region.rings {
                let mut run = Vec::new();
                for edge in ring.windows(2) {
                    let [a, b] = [edge[0], edge[1]].map(|p| {
                        project(GeoPoint {
                            lon: p.lon + shift,
                            lat: p.lat,
                        })
                    });
                    if let Some((a, b)) = clip_segment(a, b, clip) {
                        if run.last().is_some_and(|&last| last != a) {
                            flush(painter, &mut run, stroke);
                        }
                        if run.is_empty() {
                            run.push(a);
                        }
                        run.push(b);
                    } else {
                        flush(painter, &mut run, stroke);
                    }
                }
                flush(painter, &mut run, stroke);
            }
        }
    }
}
fn flush(painter: &egui::Painter, run: &mut Vec<Pos2>, stroke: Stroke) {
    if run.len() >= 2 {
        painter.add(Shape::line(std::mem::take(run), stroke));
    }
    run.clear();
}
fn clip_segment(a: Pos2, b: Pos2, r: Rect) -> Option<(Pos2, Pos2)> {
    if !a.is_finite() || !b.is_finite() {
        return None;
    }
    let d = b - a;
    let mut lo = 0.0_f32;
    let mut hi = 1.0_f32;
    for (p, q) in [
        (-d.x, a.x - r.min.x),
        (d.x, r.max.x - a.x),
        (-d.y, a.y - r.min.y),
        (d.y, r.max.y - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else if p < 0.0 {
            lo = lo.max(q / p);
        } else {
            hi = hi.min(q / p);
        }
        if lo > hi {
            return None;
        }
    }
    Some((a + d * lo, a + d * hi))
}
fn clip_polygon(mut points: Vec<Pos2>, rect: Rect) -> Vec<Pos2> {
    if points.iter().any(|p| !p.is_finite()) {
        return Vec::new();
    }
    for (axis, limit, lower) in [
        (0, rect.min.x, true),
        (0, rect.max.x, false),
        (1, rect.min.y, true),
        (1, rect.max.y, false),
    ] {
        let old = std::mem::take(&mut points);
        if old.is_empty() {
            break;
        }
        let component = |p: Pos2| if axis == 0 { p.x } else { p.y };
        let inside = |p: Pos2| {
            if lower {
                component(p) >= limit
            } else {
                component(p) <= limit
            }
        };
        let mut a = *old.last().unwrap();
        for b in old {
            if inside(a) != inside(b) {
                let t = (limit - component(a)) / (component(b) - component(a));
                points.push(a + (b - a) * t);
            }
            if inside(b) {
                points.push(b);
            }
            a = b;
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admin_border_crossing_view_survives_offscreen_endpoints() {
        let rect = Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(10., 10.));
        assert_eq!(
            clip_segment(egui::pos2(-100., 5.), egui::pos2(100., 5.), rect),
            Some((egui::pos2(0., 5.), egui::pos2(10., 5.)))
        );
        assert!(clip_segment(egui::pos2(-100., -5.), egui::pos2(100., -5.), rect).is_none());
        let polygon = clip_polygon(
            vec![
                egui::pos2(-100., -100.),
                egui::pos2(100., -100.),
                egui::pos2(0., 100.),
            ],
            rect,
        );
        assert!(polygon.len() >= 4);
        assert!(polygon.iter().all(|p| rect.contains(*p)));
    }
}
