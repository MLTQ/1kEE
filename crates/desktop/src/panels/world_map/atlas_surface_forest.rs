//! Bounded upload of one forest coverage image for an exact terrain source.
use super::super::super::atlas_forest::Mask;
use super::texture;
use eframe::wgpu;
use std::sync::Arc;

pub(super) struct Forest {
    pub texture: wgpu::Texture,
    data: Option<Arc<Mask>>,
    row: usize,
}
impl Forest {
    pub fn empty(device: &wgpu::Device) -> Self {
        Self {
            texture: texture(
                device,
                [1, 1],
                wgpu::TextureFormat::R8Unorm,
                wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
            data: None,
            row: 0,
        }
    }
    /// Returns true when the source bind group needs the new texture view.
    pub fn stage(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mask: Arc<Mask>,
        budget: &mut usize,
    ) -> bool {
        let changed = self.data.as_ref().is_none_or(|data| data.id != mask.id);
        if changed {
            self.texture = texture(
                device,
                mask.size.map(|v| v as u32),
                wgpu::TextureFormat::R8Unorm,
                wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            self.data = Some(mask.clone());
            self.row = 0;
        }
        while self.row < mask.size[1] {
            let rows = (512 * 1024 / mask.size[0])
                .max(1)
                .min(mask.size[1] - self.row);
            let bytes = rows * mask.size[0];
            if bytes > *budget {
                break;
            }
            let start = self.row * mask.size[0];
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: self.row as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &mask.pixels[start..start + bytes],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(mask.size[0] as u32),
                    rows_per_image: Some(rows as u32),
                },
                wgpu::Extent3d {
                    width: mask.size[0] as u32,
                    height: rows as u32,
                    depth_or_array_layers: 1,
                },
            );
            self.row += rows;
            *budget -= bytes;
        }
        changed
    }
    pub fn ready(&self, mask: &Mask) -> bool {
        self.data.as_ref().is_some_and(|data| data.id == mask.id) && self.row == mask.size[1]
    }
}
