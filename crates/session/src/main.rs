//! olo-session (see lib.rs).

fn main() {
    olo_crash::install();
    // (First, as in every program: in the trampoline below too, which the
    // exec replaces, so its handlers go with it.)
    // `olo-session --exec PROGRAM ARGS…`: Olo's login screen starts the
    // person's session through this, the system's copy (an ordinary
    // program, `bin_t`), as GDM does through gdm-wayland-session: SELinux
    // moves the session from the login screen's domain (`xdm_t`) into the
    // person's as it starts a program of the system's, not one in their
    // home (`~/.local/bin`, which the login screen's domain may not run).
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--exec") && args.len() > 2 {
        use std::os::unix::process::CommandExt as _;
        let e = std::process::Command::new(&args[2]).args(&args[3..]).exec();
        eprintln!("olo-session: {}: {e}", args[2].to_string_lossy());
        std::process::exit(127);
    }
    olo_session::run();
}
