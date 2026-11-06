# gpu_color.rs

## Purpose
Keep native map callbacks consistent with egui's Color32 and framebuffer policy.
egui prefers Unorm targets containing display RGB; linearizing their palette
bytes on upload changes orange to red and darkens other midtone channels.

## Contracts
- Instance bytes and live line palettes retain Color32's exact stored channels.
- `rgba` normalizes those bytes, including alpha, without a transfer curve.
- `shader` prepends the shared color-boundary WGSL to native map shaders.
- `fragment_options` specializes the shader for the actual target format.
- Linear-light backdrop shading retains its own input conversion; final output
  is encoded for the framebuffer, while display-space lines follow egui.
- CPU fallback and GPU lines must match, including palette changes and fades.
