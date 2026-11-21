static POSSIBLE_BACKENDS: &[&str] = &[
    #[cfg(feature = "winit")]
    "--winit : Run nested inside another Wayland session (development).",
    #[cfg(feature = "udev")]
    "--tty-udev : Run as the session compositor on a TTY (via logind/libseat).",
    #[cfg(feature = "udev")]
    "--headless [WxH] : Render offscreen with no display (automated UI tests).",
    #[cfg(feature = "udev")]
    "--probe-gpus : List the DRM devices and what each can do (safe beside a running desktop).",
    "--probe-shared [FOLDER [SIZES]] : Draw a window's picture on each GPU and compose it on each other, as for a display on another GPU; say what differs, keeping the pictures in FOLDER; SIZES as 964x507,333x217 (safe beside a running desktop).",
    "--plymouth-theme DIR [--scale S] : Make Olo's boot splash (a Plymouth theme, plymouth.rs) in DIR.",
];

#[cfg(feature = "profile-with-tracy-mem")]
#[global_allocator]
static GLOBAL: profiling::tracy_client::ProfiledAllocator<std::alloc::System> = profiling::tracy_client::ProfiledAllocator::new(std::alloc::System, 10);

// Allow in this function because of existing usage
#[allow(clippy::uninlined_format_args)]
fn main() {
    olo_crash::install();
    // The capability the person's compositor is started with (the GPU's
    // high priority: `olo_system::caps`) is its own: nothing it starts
    // has it.
    olo_system::caps::hand_on_nothing();
    // Started in Olo Rescue (the session's, after app_server kept
    // failing): taken out of the environment, before any thread is, so
    // what it starts (and itself, restarting) doesn't.
    if std::env::var_os(olo_protocol::rescue::START_ENV).is_some() {
        // SAFETY: no other thread yet.
        unsafe { std::env::remove_var(olo_protocol::rescue::START_ENV) };
        app_server::rescue::start_in_rescue();
    }
    // NVIDIA's EGL library kept out of this program where its card only
    // sits beside the GPU everything is drawn on (`egl_vendor`).
    // SAFETY: no other thread yet.
    let egl_vendor = unsafe { app_server::egl_vendor::choose(::std::env::args().nth(1).as_deref() == Some("--headless")) };
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    // The boot splash made (no compositor: nothing below runs).
    let args: Vec<String> = ::std::env::args().collect();
    if args.iter().any(|a| a == "--plymouth-theme") {
        std::process::exit(app_server::plymouth::run(&args));
    }
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().compact().with_env_filter(env_filter).init();
    if let Some(vendor) = &egl_vendor {
        tracing::info!(vendor, "EGL: Mesa's library alone (NVIDIA's card only sits by)");
    }

    // Every input event and every frame go through this thread: above
    // everything else (olo_protocol::performance::tier). Only the
    // person's own: a headless one (an agent's screen, a test session) is
    // background work beside it, and stays at its nice.
    // (Raised as every Olo program's is, first thing: `olo_crash::install`.)
    if let Some((was, now)) = olo_crash::open_files() {
        tracing::info!(files = now, was, "the most files this may keep open (every window's buffers and every program's connection are files)");
    }
    let headless = ::std::env::args().nth(1).as_deref() == Some("--headless");
    if headless {
        tracing::info!("the main thread stays at its nice: headless, beside the person's desktop");
    } else {
        match olo_system::priority::raise_self(olo_protocol::performance::CORE_NICE) {
            Ok(nice) => tracing::info!(nice, "the main thread's priority (rtkit)"),
            Err(e) => tracing::info!("the main thread stays at its nice: {e}"),
        }
    }
    // Realtime besides (the nice is where it goes back to): no load, in
    // any program, under any scheduler, holds the desktop up.
    if app_server::realtime::wanted(headless) {
        match app_server::realtime::promote_self(app_server::realtime::MAIN_PRIORITY) {
            Ok(()) => tracing::info!(priority = app_server::realtime::MAIN_PRIORITY, "the main thread is realtime"),
            Err(e) => tracing::info!("the main thread isn't realtime: {e}"),
        }
        // ...and the program with the keyboard, and Deskbar, too.
        app_server::realtime::enable();
        // Its own memory in RAM for good: a page of the loop's read back
        // from disk, or from swap, is a wait for everything on screen.
        match olo_system::realtime::lock_memory() {
            Ok(()) => tracing::info!("app_server's memory is kept in RAM, never swapped"),
            Err(e) => tracing::info!("app_server's memory isn't kept in RAM: {e}"),
        }
    }
    // ...and, the person's, on a core of its own (`Settings::keep_core`,
    // applied with the settings).
    if !headless {
        app_server::realtime::persons();
        app_server::realtime::home_self();
    }
    if !headless {
        // ...and on a fast core at a high clock, not where the scheduler
        // puts a mostly idle thread.
        let util_min = std::env::var("OLO_UTIL_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(olo_protocol::performance::CORE_UTIL_MIN);
        if util_min > 0
            && let Err(e) = olo_system::priority::util_min_self(util_min)
        {
            tracing::info!("the main thread keeps the scheduler's choice of core: {e}");
        }
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
        Some("--probe-shared") => app_server::udev::probe_shared(::std::env::args().nth(2).as_deref(), ::std::env::args().nth(3).as_deref()),
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
