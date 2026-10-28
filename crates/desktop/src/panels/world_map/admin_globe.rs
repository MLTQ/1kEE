//! Camera-independent administrative geometry for the spherical Earth view.
use super::{
    admin_globe_pass, admin_layer,
    contour_pass::{ContourCallback, ContourLayer, SegmentInstance},
    globe_scene::GlobeLayout,
};
use crate::model::{ActiveBody, AppModel, GeoPoint};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};

pub(super) struct Batch {
    pub version: u64,
    pub lines: Arc<Vec<SegmentInstance>>,
    pub triangles: Arc<Vec<admin_globe_pass::Triangle>>,
    _source: Arc<admin_layer::AdminData>,
}
type Cache = super::layer_snapshot::Snapshot<(usize, [egui::Color32; 10]), Batch>;
pub(super) fn draw(painter: &egui::Painter, layout: &GlobeLayout, model: &AppModel) {
    if !model.show_admin || model.active_body != ActiveBody::Earth {
        return;
    }
    painter
        .ctx()
        .request_repaint_after(std::time::Duration::from_secs(3));
    let Some(data) = admin_layer::get_for(model) else {
        return;
    };
    let colors = std::array::from_fn(|i| crate::theme::admin_color(i as u8 + 2));
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let key = (Arc::as_ptr(&data) as usize, colors);
    let batch = CACHE.get_or_init(Default::default).lock().unwrap().get(
        key,
        "admin-globe-geometry",
        |a, b| a == b,
        move |_| Some(prepare(data, colors)),
    );
    let Some(batch) = batch else {
        return;
    };
    paint(
        painter,
        layout,
        &model.globe_view,
        &batch,
        model.admin_options.shade,
        model.admin_options.opacity,
    );
}
pub(super) fn paint(
    painter: &egui::Painter,
    layout: &GlobeLayout,
    view: &crate::model::GlobeViewState,
    batch: &Batch,
    shade: bool,
    opacity: f32,
) {
    if shade {
        painter.add(admin_globe_pass::Callback::callback(
            batch.version,
            batch.triangles.clone(),
            layout,
            view,
            opacity,
            painter.ctx().pixels_per_point(),
            painter.clip_rect(),
        ));
    }
    painter.add(
        ContourCallback::new(
            ContourLayer::Admin,
            batch.version,
            batch.lines.clone(),
            layout,
            view,
            0.0,
            1.0,
            1.2,
            painter.ctx().pixels_per_point(),
        )
        .into_paint_callback(painter.clip_rect()),
    );
}
pub(super) fn prepare(data: Arc<admin_layer::AdminData>, colors: [egui::Color32; 10]) -> Batch {
    static VERSION: AtomicU64 = AtomicU64::new(1);
    let mut triangles = Vec::new();
    let mut lines = Vec::new();
    for region in &data.regions {
        if let Some(fill) = &region.fill {
            let base = colors[(region.level - 2) as usize];
            let mix = (region.relation_id as u64).wrapping_mul(0x9e3779b97f4a7c15);
            let weight = 0.65 + ((mix >> 56) as f32 / 255.0) * 0.35;
            let color = egui::Color32::from_rgb(
                (base.r() as f32 * weight) as u8,
                (base.g() as f32 * weight) as u8,
                (base.b() as f32 * weight) as u8,
            );
            for t in fill.indices.chunks_exact(3) {
                subdivide(
                    [
                        fill.vertices[t[0] as usize],
                        fill.vertices[t[1] as usize],
                        fill.vertices[t[2] as usize],
                    ],
                    &mut |points| triangles.push(admin_globe_pass::Triangle::new(points, color)),
                );
            }
        }
    }
    // Fine divisions first, country outlines last, matching Local view.
    for level in (2..=11).rev() {
        for region in data.regions.iter().filter(|r| r.level == level) {
            for ring in &region.rings {
                for pair in ring.windows(2) {
                    let steps = (span(pair[0], pair[1]) / 2.0).ceil().max(1.0) as usize;
                    for i in 0..steps {
                        lines.push(SegmentInstance::line(
                            lerp(pair[0], pair[1], i as f32 / steps as f32),
                            lerp(pair[0], pair[1], (i + 1) as f32 / steps as f32),
                            colors[(level - 2) as usize],
                        ));
                    }
                }
            }
        }
    }
    Batch {
        version: VERSION.fetch_add(1, Ordering::Relaxed),
        lines: Arc::new(lines),
        triangles: Arc::new(triangles),
        _source: data,
    }
}
fn span(a: GeoPoint, b: GeoPoint) -> f32 {
    (a.lat - b.lat).abs().max((a.lon - b.lon).abs())
}
fn lerp(a: GeoPoint, b: GeoPoint, t: f32) -> GeoPoint {
    GeoPoint {
        lat: a.lat + (b.lat - a.lat) * t,
        lon: a.lon + (b.lon - a.lon) * t,
    }
}
/// Longest-edge bisection in the loader's unwrapped lon/lat plane preserves the
/// exact even-odd tessellation (including holes), then conforms it to the sphere.
fn subdivide(t: [GeoPoint; 3], emit: &mut impl FnMut([GeoPoint; 3])) {
    let lengths = [span(t[0], t[1]), span(t[1], t[2]), span(t[2], t[0])];
    let edge = (0..3)
        .max_by(|&a, &b| lengths[a].total_cmp(&lengths[b]))
        .unwrap();
    if lengths[edge] <= 2.0 {
        emit(t);
        return;
    }
    let (a, b, c) = (t[edge], t[(edge + 1) % 3], t[(edge + 2) % 3]);
    let midpoint = lerp(a, b, 0.5);
    subdivide([a, midpoint, c], emit);
    subdivide([midpoint, b, c], emit);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spherical_subdivision_preserves_area_across_dateline() {
        let p = |lon, lat| GeoPoint { lon, lat };
        let t = [p(175., -10.), p(185., -10.), p(185., 10.)];
        let area = |t: [GeoPoint; 3]| {
            ((t[1].lon - t[0].lon) * (t[2].lat - t[0].lat)
                - (t[2].lon - t[0].lon) * (t[1].lat - t[0].lat))
                .abs()
                * 0.5
        };
        let mut sum = 0.;
        let mut count = 0;
        subdivide(t, &mut |t| {
            assert!((0..3).all(|i| span(t[i], t[(i + 1) % 3]) <= 2.0));
            assert!(t.iter().all(|p| (175.0..=185.0).contains(&p.lon)));
            sum += area(t);
            count += 1;
        });
        assert!(count > 1);
        assert!((sum - area(t)).abs() < 0.001);
    }
}
