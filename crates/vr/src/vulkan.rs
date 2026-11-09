//! Share the Vulkan device and graphics queue selected by the headset with wgpu.
use anyhow::{Context, Result, ensure};
use ash::vk::{self, Handle};
use openxr as xr;
use std::ffi::{CStr, CString};
use wgpu::hal::{self, api::Vulkan};

pub struct Graphics {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub binding: xr::vulkan::SessionCreateInfo,
}

fn extensions(mut required: Vec<&'static CStr>, runtime: &str) -> Result<Vec<&'static CStr>> {
    // HAL stores static extension names. This list is built once at process startup.
    for name in runtime.split_ascii_whitespace() {
        let name = CString::new(name)?;
        if !required.contains(&name.as_c_str()) {
            required.push(Box::leak(name.into_boxed_c_str()));
        }
    }
    Ok(required)
}

impl Graphics {
    pub fn new(xr: &xr::Instance, system: xr::SystemId) -> Result<Self> {
        let req = xr.graphics_requirements::<xr::Vulkan>(system)?;
        let flags = wgpu::InstanceFlags::empty();
        // The runtime supplies the physical device and all XR-required extensions.
        // HAL adopts each raw object, then wgpu owns its lifetime through Graphics.
        unsafe {
            let entry = ash::Entry::load()?;
            let loader_version = entry
                .try_enumerate_instance_version()?
                .unwrap_or(vk::API_VERSION_1_0);
            let maximum = req.max_api_version_supported;
            let version = loader_version.min(vk::make_api_version(
                0,
                maximum.major() as u32,
                maximum.minor() as u32,
                maximum.patch(),
            ));
            let selected = xr::Version::new(
                vk::api_version_major(version) as _,
                vk::api_version_minor(version) as _,
                vk::api_version_patch(version),
            );
            ensure!(
                selected >= req.min_api_version_supported,
                "Vulkan {selected} is below the headset minimum"
            );
            let instance_ext = extensions(
                hal::vulkan::Instance::desired_extensions(&entry, version, flags)?,
                &xr.vulkan_legacy_instance_extensions(system)?,
            )?;
            let pointers: Vec<_> = instance_ext.iter().map(|name| name.as_ptr()).collect();
            let app = vk::ApplicationInfo::default()
                .application_name(c"1kEE VR")
                .api_version(version);
            let raw = entry.create_instance(
                &vk::InstanceCreateInfo::default()
                    .application_info(&app)
                    .enabled_extension_names(&pointers),
                None,
            )?;
            let vk_instance = raw.handle();
            let instance = hal::vulkan::Instance::from_raw(
                entry,
                raw,
                version,
                0,
                None,
                instance_ext,
                flags,
                false,
                None,
            )?;
            let raw = instance.shared_instance().raw_instance();
            let physical = vk::PhysicalDevice::from_raw(
                xr.vulkan_graphics_device(system, vk_instance.as_raw() as _)? as _,
            );
            let adapter = instance
                .expose_adapter(physical)
                .context("OpenXR selected a GPU unsupported by wgpu")?;
            let family = raw
                .get_physical_device_queue_family_properties(physical)
                .iter()
                .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .context("no graphics queue")? as u32;
            let features = adapter.features & wgpu::Features::TIMESTAMP_QUERY;
            let device_ext = extensions(
                adapter.adapter.required_device_extensions(features),
                &xr.vulkan_legacy_device_extensions(system)?,
            )?;
            let pointers: Vec<_> = device_ext.iter().map(|name| name.as_ptr()).collect();
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(family)
                .queue_priorities(&[1.0])];
            let mut physical_features = adapter
                .adapter
                .physical_device_features(&device_ext, features);
            let create = physical_features.add_to_device_create(
                vk::DeviceCreateInfo::default()
                    .enabled_extension_names(&pointers)
                    .queue_create_infos(&queues),
            );
            let raw_device = raw.create_device(physical, &create, None)?;
            let binding = xr::vulkan::SessionCreateInfo {
                instance: vk_instance.as_raw() as _,
                physical_device: physical.as_raw() as _,
                device: raw_device.handle().as_raw() as _,
                queue_family_index: family,
                queue_index: 0,
            };
            let hints = wgpu::MemoryHints::default();
            let opened = adapter.adapter.device_from_raw(
                raw_device,
                None,
                &device_ext,
                features,
                &hints,
                family,
                0,
            )?;
            eprintln!("OpenXR GPU: {} (Vulkan {selected})", adapter.info.name);
            let instance = wgpu::Instance::from_hal::<Vulkan>(instance);
            let adapter = instance.create_adapter_from_hal::<Vulkan>(adapter);
            let (device, queue) = adapter.create_device_from_hal::<Vulkan>(
                opened,
                &wgpu::DeviceDescriptor {
                    label: Some("1kEE OpenXR"),
                    required_features: features,
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: hints,
                },
                None,
            )?;
            Ok(Self {
                device,
                queue,
                binding,
            })
        }
    }
}

pub struct Eye {
    // Destroy wgpu views and finish any pending GPU use before releasing XR's images.
    views: Vec<wgpu::TextureView>,
    pub chain: xr::Swapchain<xr::Vulkan>,
    pub size: [u32; 2],
    device: wgpu::Device,
}

impl Eye {
    pub fn new(
        device: &wgpu::Device,
        session: &xr::Session<xr::Vulkan>,
        size: [u32; 2],
        vk_format: vk::Format,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let [width, height] = size;
        let chain = session.create_swapchain(&xr::SwapchainCreateInfo {
            create_flags: xr::SwapchainCreateFlags::EMPTY,
            usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT,
            format: vk_format.as_raw() as _,
            sample_count: 1,
            width,
            height,
            face_count: 1,
            array_size: 1,
            mip_count: 1,
        })?;
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let mut views = Vec::new();
        for image in chain.enumerate_images()? {
            // XR owns these images. The non-owning callback tells HAL to leave the
            // VkImage alive. Eye::drop drains the queue before destroying the swapchain.
            let texture = unsafe {
                let imported = hal::vulkan::Device::texture_from_raw(
                    vk::Image::from_raw(image),
                    &hal::TextureDescriptor {
                        label: Some("XR eye"),
                        size: extent,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: hal::TextureUses::COLOR_TARGET,
                        memory_flags: hal::MemoryFlags::empty(),
                        view_formats: vec![],
                    },
                    Some(Box::new(|| {})),
                );
                device.create_texture_from_hal::<Vulkan>(
                    imported,
                    &wgpu::TextureDescriptor {
                        label: Some("XR eye"),
                        size: extent,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    },
                )
            };
            views.push(texture.create_view(&Default::default()));
        }
        Ok(Self {
            views,
            chain,
            size,
            device: device.clone(),
        })
    }

    pub fn acquire(&mut self) -> Result<&wgpu::TextureView> {
        let idx = self.chain.acquire_image()?;
        self.chain.wait_image(xr::Duration::INFINITE)?;
        Ok(&self.views[idx as usize])
    }
}

impl Drop for Eye {
    fn drop(&mut self) {
        self.views.clear();
        self.device.poll(wgpu::Maintain::Wait);
    }
}
