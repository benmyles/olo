#[cfg(target_os = "macos")]
mod apple_compat;
mod blade_atlas;
mod blade_context;
mod blade_renderer;

#[cfg(target_os = "macos")]
pub(crate) use apple_compat::*;
pub(crate) use blade_atlas::*;
pub(crate) use blade_context::*;
pub(crate) use blade_renderer::*;

/// Olo: `OLO_GPU_TIMINGS` set, every pass is timed on the GPU and each
/// frame's printed to stderr (for measuring the renderer).
fn gpu_timings() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OLO_GPU_TIMINGS").is_some())
}
