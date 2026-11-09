//! Launch-time terrain budgets. Defaults preserve existing portable behavior.
use eframe::wgpu;
use std::sync::OnceLock;

#[derive(Debug)]
pub(crate) struct Settings {
    pub gpu_bytes: usize,
    pub cpu_bytes: usize,
    pub upload_bytes: usize,
    pub upload_us: u64,
    pub atlas_cpu_bytes: usize,
    pub atlas_disk_bytes: usize,
}

fn bounded(value: Option<&str>, default: usize, min: usize, max: usize) -> usize {
    value
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| (min..=max).contains(v))
        .unwrap_or(default)
}

fn setting(name: &str, default: usize, min: usize, max: usize) -> usize {
    let value = std::env::var(name).ok();
    let result = bounded(value.as_deref(), default, min, max);
    if let Some(value) = value
        && value.parse::<usize>().ok() != Some(result)
    {
        eprintln!("[1kEE GPU] invalid {name}={value:?}; using {default} (range {min}..={max})");
    }
    result
}

pub(crate) fn settings() -> &'static Settings {
    static SETTINGS: OnceLock<Settings> = OnceLock::new();
    SETTINGS.get_or_init(|| Settings {
        gpu_bytes: setting("ONEKEE_GPU_CACHE_MIB", 3072, 256, 32768) * 1024 * 1024,
        cpu_bytes: setting("ONEKEE_CPU_CACHE_MIB", 4096, 256, 65536) * 1024 * 1024,
        upload_bytes: setting("ONEKEE_UPLOAD_MIB", 4, 1, 64) * 1024 * 1024,
        upload_us: setting("ONEKEE_UPLOAD_US", 2000, 100, 8000) as u64,
        atlas_cpu_bytes: setting("ONEKEE_ATLAS_RAM_MIB", 1024, 256, 32768) * 1024 * 1024,
        atlas_disk_bytes: setting("ONEKEE_ATLAS_DISK_MIB", 8192, 512, 262144) * 1024 * 1024,
    })
}

pub(crate) fn log_settings() {
    let s = settings();
    eprintln!(
        "[1kEE GPU] terrain retention GPU={} MiB CPU={} MiB; upload={} MiB/frame, {} us submission budget",
        s.gpu_bytes / 1048576,
        s.cpu_bytes / 1048576,
        s.upload_bytes / 1048576,
        s.upload_us
    );
    eprintln!(
        "[1kEE GPU] Atlas prepared terrain: RAM={} MiB, disk={} MiB",
        s.atlas_cpu_bytes / 1048576,
        s.atlas_disk_bytes / 1048576,
    );
}

pub(crate) fn adapter_name() -> Option<String> {
    std::env::var("ONEKEE_GPU")
        .ok()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
}

pub(crate) fn select_adapter(
    adapters: &[wgpu::Adapter],
    surface: Option<&wgpu::Surface<'_>>,
    name: &str,
) -> Result<wgpu::Adapter, String> {
    let matches: Vec<_> = adapters
        .iter()
        .filter(|a| {
            a.get_info().name.to_lowercase().contains(name)
                && surface.is_none_or(|s| a.is_surface_supported(s))
        })
        .collect();
    // The same GPU can be exposed by Vulkan and GL. Prefer its native backend.
    matches
        .into_iter()
        .min_by_key(|a| match a.get_info().backend {
            wgpu::Backend::Vulkan | wgpu::Backend::Metal | wgpu::Backend::Dx12 => 0,
            _ => 1,
        })
        .cloned()
        .ok_or_else(|| {
            format!(
                "ONEKEE_GPU={name:?} matched no compatible adapter. Available: {}",
                adapters
                    .iter()
                    .map(|a| a.get_info().name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_budgets_cannot_disable_progress_or_allocate_unbounded_memory() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("-1"),
            Some("1.5"),
            Some("65"),
            Some("18446744073709551616"),
        ] {
            assert_eq!(bounded(value, 4, 1, 64), 4);
        }
        assert_eq!(bounded(Some("1"), 4, 1, 64), 1);
        assert_eq!(bounded(Some("64"), 4, 1, 64), 64);
    }
}
