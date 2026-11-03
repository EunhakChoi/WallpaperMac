//! Native macOS application and embeddable wallpaper surfaces.
use std::path::PathBuf;
pub mod editor_requests;
#[derive(Debug, Default, Clone)]
pub struct Options {
    pub project: Option<PathBuf>,
    pub assets: Option<PathBuf>,
    pub desktop: bool,
    pub screen: usize,
}
#[cfg(target_os = "macos")]
mod native;
#[cfg(any(target_os = "macos", test))]
pub mod render_graph;
#[cfg(target_os = "macos")]
pub use native::embedded::{SurfaceOptions, WallpaperSurface};
#[cfg(target_os = "macos")]
pub fn run(options: Options) -> Result<(), String> {
    objc2::rc::autoreleasepool(|_| native::run(options))
}

#[cfg(target_os = "macos")]
pub fn screen_identity(screen: &objc2_app_kit::NSScreen) -> String {
    native::screen_identity(screen)
}

#[cfg(target_os = "macos")]
pub use native::metal::offscreen::{GpuReadback, OffscreenRenderer};

#[cfg(target_os = "macos")]
pub fn metal_smoke(
    output: &std::path::Path,
    shader_root: Option<&std::path::Path>,
    mtm: objc2::MainThreadMarker,
) -> Result<serde_json::Value, String> {
    native::metal::smoke::run(output, shader_root, mtm)
}

#[cfg(target_os = "macos")]
pub fn local_time_of_day() -> f32 {
    native::local_time_of_day()
}
