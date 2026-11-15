mod blade_atlas;
mod blade_context;
mod blade_renderer;

pub(crate) use blade_atlas::*;
pub(crate) use blade_context::*;
pub(crate) use blade_renderer::*;

/// `OLO_GPU_TIMINGS` set, every pass is timed on the GPU and each
/// frame's printed to stderr (for measuring the renderer).
/// The GPU's lowest priority for this program's drawing: on an agent's
/// screen (`OLO_SCREEN`), as its threads stay at their nice and its
/// compositor asks the same, so the person's desktop draws first.
/// `OLO_GPU_PRIORITY=low` or `normal` says instead (a test session).
fn gpu_low_priority() -> bool {
    match std::env::var("OLO_GPU_PRIORITY").as_deref() {
        Ok("low") => true,
        Ok(_) => false,
        Err(_) => std::env::var_os("OLO_SCREEN").is_some(),
    }
}

fn gpu_timings() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OLO_GPU_TIMINGS").is_some())
}
