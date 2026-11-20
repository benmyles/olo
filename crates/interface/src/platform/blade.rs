mod blade_atlas;
mod blade_context;
mod blade_renderer;

pub(crate) use blade_atlas::*;
pub(crate) use blade_context::*;
pub(crate) use blade_renderer::*;

/// `OLO_GPU_TIMINGS` set, every pass is timed on the GPU and each
/// frame's printed to stderr (for measuring the renderer).
// (No program of Olo's asks the GPU for a priority below the ordinary
// one. A kit program on an agent's screen did, from 2026-11-15 to
// 2026-11-20 (`VK_KHR_global_priority`'s LOW), as its compositor did:
// a queue below ordinary gets nothing while any ordinary one has work,
// and Intel's xe resets the whole GPU when a job has waited five
// seconds to start. app_server's `headless::gpu_priority` has the
// measurements.)

fn gpu_timings() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OLO_GPU_TIMINGS").is_some())
}
