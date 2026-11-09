//! Forest topology is tessellated on the map plane, before height projection.
use crate::model::GeoPoint;
use lyon_tessellation::path::{Path, builder::PathBuilder, math::point};
use lyon_tessellation::{BuffersBuilder, FillOptions, FillTessellator, FillVertex, VertexBuffers};

pub(super) struct Fill {
    pub vertices: Vec<(GeoPoint, f32)>,
    pub indices: Vec<u32>,
}

/// Unwrap consecutive edges once; a frame shifts the whole feature near focus.
pub(super) fn unwrap(points: &mut [(GeoPoint, f32)]) {
    let Some((first, _)) = points.iter().find(|(p, _)| p.lon.is_finite()) else {
        return;
    };
    let mut previous = first.lon;
    for (p, _) in points {
        if p.lon.is_finite() {
            p.lon += ((previous - p.lon) / 360.0).round() * 360.0;
            previous = p.lon;
        }
    }
}

pub(super) fn tessellate(points: &[(GeoPoint, f32)]) -> Option<Fill> {
    if points.len() < 4
        || points.first()?.0 != points.last()?.0
        || points
            .iter()
            .any(|(p, h)| !p.lat.is_finite() || !p.lon.is_finite() || !h.is_finite())
    {
        return None;
    }
    let origin = points[0].0;
    // Local metres avoid degree-scale tolerances and large absolute coordinates.
    let x_scale = 111_320.0 * origin.lat.to_radians().cos().abs().max(0.05);
    let y_scale = 111_320.0;
    let at = |p: GeoPoint| {
        point(
            (p.lon - origin.lon) * x_scale,
            (p.lat - origin.lat) * y_scale,
        )
    };
    let mut builder = Path::builder_with_attributes(1);
    builder.begin(at(origin), &[points[0].1]);
    let mut previous = origin;
    for &(p, h) in &points[1..points.len() - 1] {
        if p != previous {
            builder.line_to(at(p), &[h]);
            previous = p;
        }
    }
    builder.close();
    let mut buffers: VertexBuffers<(GeoPoint, f32), u32> = VertexBuffers::new();
    FillTessellator::new()
        .tessellate_path(
            &builder.build(),
            &FillOptions::even_odd(),
            &mut BuffersBuilder::new(&mut buffers, |mut v: FillVertex<'_>| {
                let p = v.position();
                let h = v.interpolated_attributes()[0];
                (
                    GeoPoint {
                        lon: origin.lon + p.x / x_scale,
                        lat: origin.lat + p.y / y_scale,
                    },
                    h,
                )
            }),
        )
        .ok()?;
    (!buffers.indices.is_empty()).then_some(Fill {
        vertices: buffers.vertices,
        indices: buffers.indices,
    })
}

/// Preserve each triangle's original indices. An invalid projection drops only
/// that triangle; filtering/reconnecting its surviving vertices would invent land.
pub(super) fn project_fill(
    fill: &Fill,
    color: egui::Color32,
    clip: egui::Rect,
    project: impl Fn(GeoPoint, f32) -> Option<egui::Pos2>,
) -> egui::Mesh {
    let positions = fill
        .vertices
        .iter()
        .map(|&(p, h)| project(p, h))
        .collect::<Vec<_>>();
    let mut mesh = egui::Mesh::default();
    for triangle in fill.indices.chunks_exact(3) {
        let [Some(a), Some(b), Some(c)] = [
            positions[triangle[0] as usize],
            positions[triangle[1] as usize],
            positions[triangle[2] as usize],
        ] else {
            continue;
        };
        if !a.is_finite()
            || !b.is_finite()
            || !c.is_finite()
            || !egui::Rect::from_points(&[a, b, c]).intersects(clip)
        {
            continue;
        }
        let start = mesh.vertices.len() as u32;
        for p in [a, b, c] {
            mesh.colored_vertex(p, color);
        }
        mesh.add_triangle(start, start + 1, start + 2);
    }
    mesh
}

/// Open path strokes cut sharp corners safely. Repeating the first source point
/// closes the geographic ring without invoking egui's unbounded closed miters.
pub(super) fn outlines(
    points: &[(GeoPoint, f32)],
    stroke: egui::Stroke,
    project: impl Fn(GeoPoint, f32) -> Option<egui::Pos2>,
) -> Vec<egui::Shape> {
    let mut shapes = Vec::new();
    let mut run = Vec::new();
    let flush = |run: &mut Vec<egui::Pos2>, shapes: &mut Vec<egui::Shape>| {
        if run.len() >= 2 {
            shapes.push(egui::Shape::line(std::mem::take(run), stroke));
        }
        run.clear();
    };
    for &(p, h) in points {
        match project(p, h).filter(|p| p.is_finite()) {
            Some(p) => {
                if run.last() != Some(&p) {
                    run.push(p);
                }
            }
            None => flush(&mut run, &mut shapes),
        }
    }
    flush(&mut run, &mut shapes);
    shapes
}
