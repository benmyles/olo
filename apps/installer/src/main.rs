//! olo-installer: Olo Setup. One program, downloaded from Olo's
//! releases and run on a stock Fedora Workstation, that makes it Olo:
//! full screen, Windows 95's Setup in BeOS's look (ui.rs), a wizard of
//! five pages; what Olo needs installed from Fedora through PackageKit,
//! Olo itself downloaded, checked and put in the home folder, Olo on the
//! login screen's menu, and, if chosen, Olo's login screen, boot splash
//! and RPM Fusion (plan.rs). What needs root is Olo Doctor's privileged
//! helper, this program run again through pkexec. It can be stopped and
//! run again: each stage does only what isn't done.
//!
//! Olo is a work of love, dedicated to Sir Samwise, the bestest English
//! Labrador that ever walked the earth.
//!
//! ```text
//! olo-installer                    the window
//! olo-installer --yes [--login-screen] [--boot-splash] [--rpm-fusion]
//!                                  through the pages by itself, those
//!                                  chosen (tests; an agent)
//! olo-installer --bundle URL|FILE  that archive (its .sha256 beside it),
//!                                  not the newest release's
//! olo-installer --check            whether this computer can have Olo,
//!                                  as JSON; opens nothing
//! olo-installer --helper           the privileged helper (pkexec runs it)
//! ```

mod assets;
mod bundle;
mod plan;
mod session;
mod ui;

use olo_ui::{WindowOptions, gpui::AppContext as _};

const APP_ID: &str = "olo.installer";

fn main() {
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--helper") {
        olo_doctor::helper::serve();
    }
    if args.iter().any(|a| a == "--check") {
        let c = plan::look();
        let why = plan::unsuitable(&c);
        let v = serde_json::json!({"fedora": c.fedora, "variant": c.variant, "user": c.user, "arch": bundle::arch(), "free": c.free, "nvidia": c.nvidia, "installed": c.installed, "suitable": why.is_none(), "why": why});
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        std::process::exit(if why.is_none() { 0 } else { 1 });
    }
    let given = args.iter().position(|a| a == "--bundle").and_then(|i| args.get(i + 1).cloned());
    let auto = args.iter().any(|a| a == "--yes");
    let choices =
        plan::Choices { login_screen: args.iter().any(|a| a == "--login-screen"), boot_splash: args.iter().any(|a| a == "--boot-splash"), rpm_fusion: args.iter().any(|a| a == "--rpm-fusion") };
    // The kit's fonts and Setup's icons, from what it carries.
    if let Some(dir) = assets::unpack() {
        // SAFETY: single-threaded, before anything else starts.
        unsafe { std::env::set_var("OLO_DATA", dir) };
    }
    olo_ui::run(APP_ID, move |cx| {
        let proxy = olo_ui::proxy(cx);
        let setup = cx.new(|_| ui::Setup::new(proxy.clone(), given, auto, choices));
        let view = setup.clone();
        olo_ui::open_window(cx, WindowOptions { fullscreen: true, ..WindowOptions::new("Olo Setup", 1024.0, 768.0) }, move |_, _| view);
        let messages = setup.clone();
        olo_ui::message_handler(cx, move |message, cx| {
            if let Ok(t) = message.downcast::<ui::Told>() {
                messages.update(cx, |s, cx| s.told(t.0, cx));
            } else {
                messages.update(cx, |s, cx| s.advance(cx));
            }
        });
        // --yes: a page every few seconds (each shown long enough to see).
        if auto {
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    proxy.send(ui::Advance);
                }
            });
        }
    });
}
