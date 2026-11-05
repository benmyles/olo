static POSSIBLE_BACKENDS: &[&str] = &[
    #[cfg(feature = "winit")]
    "--winit : Run nested inside another Wayland session (development).",
    #[cfg(feature = "udev")]
    "--tty-udev : Run as the session compositor on a TTY (via logind/libseat).",
    #[cfg(feature = "udev")]
    "--headless [WxH] : Render offscreen with no display (automated UI tests).",
    #[cfg(feature = "udev")]
    "--probe-gpus : List the DRM devices and what each can do (safe beside a running desktop).",
];

#[cfg(feature = "profile-with-tracy-mem")]
#[global_allocator]
static GLOBAL: profiling::tracy_client::ProfiledAllocator<std::alloc::System> = profiling::tracy_client::ProfiledAllocator::new(std::alloc::System, 10);

/// Smithay's xdg-popup commit hook still runs for a surface whose popup
/// the client has destroyed (GTK commits it once more, as it may), finds
/// no parent, and, with no popup left to post a protocol error on, logs
/// "surface missing from known popups" as an error. Nothing is wrong, so
/// that one callsite stays out of the log; the others still show.
fn bogus_error(m: &tracing::Metadata<'_>) -> bool {
    *m.level() == tracing::Level::ERROR && m.target() == "smithay::wayland::shell::xdg" && m.file().is_some_and(|f| f.ends_with("shell/xdg/mod.rs")) && m.line() == Some(2065)
}

// Allow in this function because of existing usage
#[allow(clippy::uninlined_format_args)]
fn main() {
    olo_crash::install();
    // Started in Olo Rescue (the session's, after app_server kept
    // failing): taken out of the environment, before any thread is, so
    // what it starts (and itself, restarting) doesn't.
    if std::env::var_os(olo_protocol::rescue::START_ENV).is_some() {
        // SAFETY: no other thread yet.
        unsafe { std::env::remove_var(olo_protocol::rescue::START_ENV) };
        app_server::rescue::start_in_rescue();
    }
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().compact().with_env_filter(env_filter).finish().with(tracing_subscriber::filter::filter_fn(|m| !bogus_error(m))).init();

    // Every input event and every frame go through this thread: above
    // everything else (olo_protocol::performance::tier).
    match olo_system::priority::raise_self(olo_protocol::performance::CORE_NICE) {
        Ok(nice) => tracing::info!(nice, "the main thread's priority (rtkit)"),
        Err(e) => tracing::info!("the main thread stays at its nice: {e}"),
    }
    // ...and on a fast core at a high clock, not where the scheduler puts
    // a mostly idle thread.
    let util_min = std::env::var("OLO_UTIL_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(olo_protocol::performance::CORE_UTIL_MIN);
    if util_min > 0
        && let Err(e) = olo_system::priority::util_min_self(util_min)
    {
        tracing::info!("the main thread keeps the scheduler's choice of core: {e}");
    }

    profiling::register_thread!("Main Thread");

    let arg = ::std::env::args().nth(1);
    match arg.as_ref().map(|s| &s[..]) {
        #[cfg(feature = "winit")]
        Some("--winit") => {
            tracing::info!("Starting app_server with winit backend");
            app_server::winit::run_winit();
        }
        #[cfg(feature = "udev")]
        Some("--tty-udev") => {
            tracing::info!("Starting app_server on a tty using udev");
            app_server::udev::run_udev();
        }
        #[cfg(feature = "udev")]
        Some("--probe-gpus") => app_server::udev::probe_gpus(),
        #[cfg(feature = "udev")]
        Some("--headless") => {
            let size = ::std::env::args().nth(2);
            app_server::headless::run_headless(size.as_deref());
        }
        Some(other) => {
            tracing::error!("Unknown backend: {}", other);
        }
        None => {
            #[allow(clippy::disallowed_macros)]
            {
                println!("USAGE: olo-app-server --backend");
                println!();
                println!("Possible backends are:");
                for b in POSSIBLE_BACKENDS {
                    println!("\t{b}");
                }
            }
        }
    }
}
