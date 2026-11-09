use crate::operator::{MAP_SIZE, MapImage, WRIST_SIZE};
use glam::{Mat4, Vec2, Vec3};
use one_thousand_electric_eye_desktop::vr_operator::{HEIGHT_EDGE, MapFrame};

// All textures are allocated once. Draw the front layer while filling the back one.
struct Surface {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    size: [u32; 2],
    front: u32,
    row: u32,
    bytes: Option<Vec<u8>>,
}
impl Surface {
    fn new(device: &wgpu::Device, size: [u32; 2], label: &str) -> Self {
        let texture = array_texture(device, size, label, wgpu::TextureFormat::Rgba8UnormSrgb);
        let view = texture.create_view(&Default::default());
        Self {
            texture,
            view,
            size,
            front: 0,
            row: 0,
            bytes: None,
        }
    }
    fn advance(&mut self, queue: &wgpu::Queue, budget: usize) -> bool {
        let Some(bytes) = &self.bytes else {
            return false;
        };
        let stride = self.size[0] * 4;
        let rows = (budget as u32 / stride).max(1).min(self.size[1] - self.row);
        let end = self.row + rows;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d {
                    x: 0,
                    y: self.row,
                    z: 1 - self.front,
                },
                ..self.texture.as_image_copy()
            },
            &bytes[(self.row * stride) as usize..(end * stride) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: self.size[0],
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        self.row = end;
        if end < self.size[1] {
            return false;
        }
        self.front = 1 - self.front;
        self.row = 0;
        self.bytes = None;
        true
    }
}
fn array_texture(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 2,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}
pub struct Cartography {
    room: Surface,
    next_room: Option<Vec<u8>>,
    pub show_room: bool,
    pub room_ready: bool,
    map: Surface,
    wrist: Surface,
    heights: wgpu::Texture,
    pub height_view: wgpu::TextureView,
    pub frame: Option<MapFrame>,
    next: Option<MapImage>,
    uploading: Option<MapFrame>,
    next_wrist: Option<Vec<u8>>,
    pub wrist_ready: bool,
    pub wrist_pose: Option<Mat4>,
    pub pointer: Option<Vec2>,
}
impl Cartography {
    pub fn new(d: &wgpu::Device) -> Self {
        let heights = array_texture(
            d,
            [HEIGHT_EDGE as u32; 2],
            "local relief heights",
            wgpu::TextureFormat::R32Float,
        );
        Self {
            room: Surface::new(d, crate::operator::ROOM_SIZE, "live event and webcam"),
            next_room: None,
            show_room: false,
            room_ready: false,
            map: Surface::new(d, MAP_SIZE, "native globe and local map"),
            wrist: Surface::new(d, WRIST_SIZE, "wrist controls"),
            height_view: heights.create_view(&Default::default()),
            heights,
            frame: None,
            next: None,
            uploading: None,
            next_wrist: None,
            wrist_ready: false,
            wrist_pose: None,
            pointer: None,
        }
    }
    pub fn room_view(&self) -> &wgpu::TextureView {
        &self.room.view
    }
    pub fn room_layer(&self) -> f32 {
        self.room.front as f32
    }
    pub fn update_room(&mut self, q: &wgpu::Queue, pixels: Option<Vec<u8>>, visible: Option<bool>) {
        if let Some(v) = visible {
            self.show_room = v;
        }
        if pixels.is_some() {
            self.next_room = pixels;
        }
        if self.room.bytes.is_none() {
            self.room.bytes = self.next_room.take();
        }
        if self.room.advance(q, 256 * 1024) {
            self.room_ready = true;
        }
    }
    pub fn map_view(&self) -> &wgpu::TextureView {
        &self.map.view
    }
    pub fn wrist_view(&self) -> &wgpu::TextureView {
        &self.wrist.view
    }
    pub fn layers(&self) -> [f32; 4] {
        [
            self.map.front as f32,
            self.wrist.front as f32,
            self.frame
                .as_ref()
                .map_or(0., |f| if f.local { 2. } else { 1. }),
            f32::from(self.frame.as_ref().is_some_and(|f| !f.heights.is_empty())),
        ]
    }
    pub fn advance(&mut self, q: &wgpu::Queue, map: Option<MapImage>, wrist: Option<Vec<u8>>) {
        if let Some(m) = map {
            self.next = Some(m);
        }
        if let Some(w) = wrist {
            self.next_wrist = Some(w);
        }
        if self.wrist.bytes.is_none() {
            self.wrist.bytes = self.next_wrist.take();
        }
        if self.wrist.advance(q, 512 * 1024) {
            self.wrist_ready = true;
        }
        if self.map.bytes.is_none()
            && let Some(m) = self.next.take()
        {
            self.map.bytes = Some(m.pixels);
            self.uploading = Some(m.frame);
        }
        if self.map.advance(q, 512 * 1024) {
            self.frame = self.uploading.take();
            if let Some(f) = &self.frame
                && !f.heights.is_empty()
            {
                q.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: self.map.front,
                        },
                        ..self.heights.as_image_copy()
                    },
                    bytemuck::cast_slice(&f.heights),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * HEIGHT_EDGE as u32),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width: HEIGHT_EDGE as u32,
                        height: HEIGHT_EDGE as u32,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }
}

pub fn globe_uv(f: &MapFrame, point: Vec3) -> Option<Vec2> {
    let (sy, cy) = f.angles[0].sin_cos();
    let (sp, cp) = f.angles[1].sin_cos();
    let x = cy * point.z + sy * point.x;
    let z = cy * point.x - sy * point.z;
    let y = cp * point.y - sp * z;
    let vz = sp * point.y + cp * z;
    let [cx, cy, gain, distance] = f.projection;
    if vz * distance <= 1. {
        return None;
    }
    let uv = Vec2::new(cx, cy) - Vec2::new(x, y) * gain / (distance - vz);
    (uv.min_element() >= 0. && uv.max_element() <= 1.).then_some(uv)
}
fn height(f: &MapFrame, uv: Vec2) -> f32 {
    if f.heights.is_empty() {
        return 0.;
    }
    let p = uv.clamp(Vec2::ZERO, Vec2::ONE) * (HEIGHT_EDGE - 1) as f32;
    let (x, y) = (p.x as usize, p.y as usize);
    let sample = |dx: usize, dy: usize| {
        f.heights[(y + dy).min(HEIGHT_EDGE - 1) * HEIGHT_EDGE + (x + dx).min(HEIGHT_EDGE - 1)]
    };
    let top = sample(0, 0) + (sample(1, 0) - sample(0, 0)) * p.x.fract();
    let bottom = sample(0, 1) + (sample(1, 1) - sample(0, 1)) * p.x.fract();
    top + (bottom - top) * p.y.fract()
}
pub fn table_uv(f: &MapFrame, table: Mat4, ray: Mat4) -> Option<Vec2> {
    let m = table.inverse() * ray;
    let o = m.w_axis.truncate();
    let d = -m.z_axis.truncate().normalize();
    let bound = Vec3::new(0.5, 0.5, 0.25);
    let mut start = 0.0f32;
    let mut end = f32::MAX;
    for i in 0..3 {
        if d[i].abs() < 1e-6 {
            if o[i].abs() > bound[i] {
                return None;
            }
        } else {
            let a = (-bound[i] - o[i]) / d[i];
            let b = (bound[i] - o[i]) / d[i];
            start = start.max(a.min(b));
            end = end.min(a.max(b));
        }
    }
    if end <= start || d.z >= 0. {
        return None;
    }
    let uv = |t: f32| {
        let p = o + d * t;
        Vec2::new(p.x + 0.5, 0.5 - p.y)
    };
    let above = |t: f32| o.z + d.z * t > height(f, uv(t));
    let mut last = start;
    for i in 0..=32 {
        let t = start + (end - start) * i as f32 / 32.;
        if !above(t) {
            let (mut lo, mut hi) = (last, t);
            for _ in 0..5 {
                let mid = (lo + hi) * 0.5;
                if above(mid) { lo = mid } else { hi = mid }
            }
            return Some(uv(hi));
        }
        last = t;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn surface_pick_uses_published_camera_and_stereo_relief() {
        let globe = MapFrame {
            angles: [-std::f32::consts::FRAC_PI_2, 0.],
            projection: [0.45, 0.55, 1.1, 3.],
            ..Default::default()
        };
        assert!(
            globe_uv(&globe, Vec3::Z)
                .unwrap()
                .distance(Vec2::new(0.45, 0.55))
                < 1e-5
        );
        assert!(globe_uv(&globe, -Vec3::Z).is_none());
        let tile = MapFrame {
            local: true,
            heights: vec![0.12; HEIGHT_EDGE * HEIGHT_EDGE],
            ..Default::default()
        };
        let origin = Vec3::new(0., 0., 1.);
        let expected = Vec3::new(0.25, -0.16, 0.12);
        let ray = Mat4::look_at_rh(origin, expected, Vec3::Y).inverse();
        assert!(
            table_uv(&tile, Mat4::IDENTITY, ray)
                .unwrap()
                .distance(Vec2::new(0.75, 0.66))
                < 1e-3
        );
    }
}
