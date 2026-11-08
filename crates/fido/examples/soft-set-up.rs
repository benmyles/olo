//! Sets up the soft key (`OLO_FIDO_SOFT`) for the user running it, as
//! Preferences ▸ Security ▸ Set Up… does, sealing PASSWORD (default
//! "test") in the config directory's security-keys.json: for a test
//! session (`XDG_CONFIG_HOME` its own), where a window then waits for the
//! soft key's touch as it would for a real one's. A debug build only (the
//! soft key is never in a release one); it refuses without
//! `OLO_FIDO_SOFT`, so it never seals with a key that's plugged in.
//!
//! `OLO_FIDO_SOFT=FILE XDG_CONFIG_HOME=DIR cargo run -p olo-fido --example soft-set-up [PASSWORD]`

use std::sync::atomic::AtomicBool;

fn main() {
    if std::env::var_os("OLO_FIDO_SOFT").is_none() || !cfg!(debug_assertions) {
        eprintln!("soft-set-up: a debug build with OLO_FIDO_SOFT=FILE only");
        std::process::exit(2);
    }
    let password = std::env::args().nth(1).unwrap_or_else(|| "test".into());
    let user = olo_system_user();
    let Some(device) = olo_fido::devices().into_iter().find(|d| d.path.as_os_str() == "soft") else {
        eprintln!("soft-set-up: no soft key");
        std::process::exit(1);
    };
    let sealed = olo_fido::set_up(&device, &user, &password, None, &AtomicBool::new(false), &|_| {}).unwrap_or_else(|e| {
        eprintln!("soft-set-up: {e}");
        std::process::exit(1);
    });
    let mut keys = olo_fido::seal::load(&user);
    keys.push(sealed);
    olo_fido::seal::save(&user, &keys).unwrap_or_else(|e| {
        eprintln!("soft-set-up: {e}");
        std::process::exit(1);
    });
    println!("{}: {} key(s) set up for {user}", olo_fido::seal::path().display(), keys.len());
}

/// The login name, as `olo_system::auth::current_user` finds it.
fn olo_system_user() -> String {
    // SAFETY: getpwuid returns a pointer into static storage (or null),
    // copied out at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned()
    }
}
