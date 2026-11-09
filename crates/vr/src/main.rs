#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod brief;
#[cfg(target_os = "linux")]
mod input;
#[cfg(target_os = "linux")]
mod render;
#[cfg(target_os = "linux")]
mod scene;
#[cfg(target_os = "linux")]
mod vulkan;
#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    app::main()
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("1kEE VR currently requires Linux with Vulkan and an OpenXR runtime.");
    std::process::exit(1);
}
