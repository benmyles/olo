static POSSIBLE_BACKENDS: &[&str] = &[
    #[cfg(feature = "winit")]
    "--winit : Run nested inside another Wayland session (development).",
    #[cfg(feature = "udev")]
    "--tty-udev : Run as the session compositor on a TTY (via logind/libseat).",
    #[cfg(feature = "udev")]
    "--headless [WxH] : Render offscreen with no display (automated UI tests).",
];

#[cfg(feature = "profile-with-tracy-mem")]
#[global_allocator]
static GLOBAL: profiling::tracy_client::ProfiledAllocator<std::alloc::System> =
    profiling::tracy_client::ProfiledAllocator::new(std::alloc::System, 10);

// Allow in this function because of existing usage
#[allow(clippy::uninlined_format_args)]
fn main() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt()
            .compact()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt().compact().init();
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
                println!("USAGE: tabula-app-server --backend");
                println!();
                println!("Possible backends are:");
                for b in POSSIBLE_BACKENDS {
                    println!("\t{b}");
                }
            }
        }
    }
}
