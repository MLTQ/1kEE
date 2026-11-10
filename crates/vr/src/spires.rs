//! Instanced geometry and controller hit tests share the published marker axes.
use crate::{cartography::height, scene::surface};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};
use one_thousand_electric_eye_desktop::vr_operator::{HEIGHT_EDGE, MapFrame, Spire, Target};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Instance {
    pub base: [f32; 3],
    pub radius: f32,
    pub tip: [f32; 3],
    pub color: [u8; 4],
}
impl Instance {
    fn new(f: &MapFrame, marker: &Spire) -> Self {
        let [lat, lon] = marker.location;
        let camera = matches!(marker.target, Some(Target::Camera(_)));
        let scale = f.marker_scale / if f.local { 1. } else { f.globe_scale.max(1.) };
        let (base, normal, extent) = if f.local {
            let [w, s, e, n] = f.bounds;
            let uv = Vec2::new((lon - w) / (e - w), (n - lat) / (n - s));
            let step = 1. / (HEIGHT_EDGE - 1) as f32;
            let low = (uv - step).max(Vec2::ZERO);
            let high = (uv + step).min(Vec2::ONE);
            let dx = (height(f, Vec2::new(high.x, uv.y)) - height(f, Vec2::new(low.x, uv.y)))
                / (high.x - low.x);
            let dy = (height(f, Vec2::new(uv.x, high.y)) - height(f, Vec2::new(uv.x, low.y)))
                / (high.y - low.y);
            (
                Vec3::new(uv.x - 0.5, 0.5 - uv.y, height(f, uv)),
                Vec3::new(-dx, dy, 1.).normalize(),
                0.06,
            )
        } else {
            let normal = surface(lat, lon);
            (normal, normal, 0.12)
        };
        let length = extent * scale * if camera { 0.65 } else { 1. };
        let [r, g, b] = marker.color;
        Self {
            base: base.to_array(),
            radius: extent * scale * 0.085,
            tip: (base + normal * length).to_array(),
            color: [r, g, b, 255],
        }
    }
}

pub struct Spires {
    device: wgpu::Device,
    pub buffer: wgpu::Buffer,
    pub instances: Vec<Instance>,
}
impl Spires {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            device: device.clone(),
            buffer: Self::buffer(device, 1),
            instances: vec![],
        }
    }
    fn buffer(device: &wgpu::Device, count: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Geographic spires"),
            size: (count * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        })
    }
    pub fn publish(&mut self, q: &wgpu::Queue, f: &MapFrame) {
        self.instances.clear();
        self.instances
            .extend(f.spires.iter().map(|m| Instance::new(f, m)));
        let bytes = bytemuck::cast_slice(&self.instances);
        if bytes.len() as u64 > self.buffer.size() {
            self.buffer = Self::buffer(&self.device, self.instances.len().next_power_of_two());
        }
        if !bytes.is_empty() {
            q.write_buffer(&self.buffer, 0, bytes);
        }
    }

    /// Find the nearest illuminated shaft before the map surface. A small
    /// pointing tolerance around it makes its narrow tip selectable.
    pub fn pick(&self, ray: Mat4, limit: f32, frame: &MapFrame) -> Option<(usize, f32)> {
        let o = ray.w_axis.truncate();
        let d = -ray.z_axis.truncate().normalize();
        self.instances
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                frame.spires[i].target.as_ref()?;
                let a = Vec3::from_array(m.base);
                let axis = Vec3::from_array(m.tip) - a;
                let da = d.dot(axis);
                let delta = a - o;
                let det = axis.length_squared() - da * da;
                let u = if det > 1e-10 {
                    ((da * d.dot(delta) - axis.dot(delta)) / det).clamp(0., 1.)
                } else if da < 0. {
                    1.
                } else {
                    0.
                };
                let closest = delta + u * axis;
                let t = closest.dot(d);
                let dist_sq = (closest - t * d).length_squared();
                let radius = m.radius * (2.2 - u * 0.5);
                if t <= 0. || dist_sq > radius * radius {
                    return None;
                }
                let near = (t - (radius * radius - dist_sq).sqrt()).max(0.);
                (near <= limit).then_some((i, near))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_spire_rises_perpendicular_to_the_published_height_field() {
        // In table coordinates x points east and y north, while texture v increases south.
        let heights = (0..HEIGHT_EDGE * HEIGHT_EDGE)
            .map(|i| {
                let u = (i % HEIGHT_EDGE) as f32 / (HEIGHT_EDGE - 1) as f32;
                let v = (i / HEIGHT_EDGE) as f32 / (HEIGHT_EDGE - 1) as f32;
                0.1 * u - 0.08 * v
            })
            .collect();
        let frame = MapFrame {
            local: true,
            bounds: [-1., -1., 1., 1.],
            marker_scale: 1.,
            heights,
            ..Default::default()
        };
        let marker = Spire {
            location: [0., 0.],
            color: [255; 3],
            target: Some(Target::Camera("cam".into())),
        };
        let spire = Instance::new(&frame, &marker);
        let base = Vec3::from_array(spire.base);
        let axis = (Vec3::from_array(spire.tip) - base).normalize();
        assert!(base.distance(Vec3::new(0., 0., 0.01)) < 1e-6);
        assert!(axis.dot(Vec3::new(1., 0., 0.1)).abs() < 1e-5);
        assert!(axis.dot(Vec3::new(0., 1., 0.08)).abs() < 1e-5);
        assert!(axis.z > 0.);
    }
}
