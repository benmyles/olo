//! The store opened by the login password, with nothing of GNOME's: on a
//! private bus with no keyring but olo-secrets, each run in a login
//! keyring of its own (as each login has, holding the key PAM's
//! pam_olo_keyring gave it). The first makes the store and seals its key
//! by that login's password; the next, the same password, opens it; one
//! whose password differs (changed since) can't without asking, and
//! there's no one here to ask, so it says so and leaves the seal as it was.

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn join_new_session_keyring() {
    // SAFETY: KEYCTL_JOIN_SESSION_KEYRING with no name: a new anonymous one.
    assert!(unsafe { libc::syscall(libc::SYS_keyctl, 1 as libc::c_long, std::ptr::null::<libc::c_char>()) } > 0);
}

fn give_login_key(key: &[u8; 32]) {
    let name = std::ffi::CString::new(olo_login_key::KEY_NAME).unwrap();
    // SAFETY: add_key with NUL-terminated strings and a 32-byte payload.
    assert!(unsafe { libc::syscall(libc::SYS_add_key, c"user".as_ptr(), name.as_ptr(), key.as_ptr(), 32 as libc::size_t, -3 as libc::c_long) } > 0);
}

fn status(dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(dir.join("run/olo-secrets.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// olo-secrets run once in a new login keyring holding `login`, until it
/// says how it went.
fn run(dir: &Path, bus: &str, login: &[u8; 32]) -> serde_json::Value {
    join_new_session_keyring();
    give_login_key(login);
    let _ = std::fs::remove_file(dir.join("run/olo-secrets.json"));
    let mut child: Child = Command::new(env!("CARGO_BIN_EXE_olo-secrets"))
        .env("DBUS_SESSION_BUS_ADDRESS", bus)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("OLO_SECRETS_SERVE", "1")
        .env("OLO_SECRETS_STORE", dir.join("secrets.store"))
        .env_remove("OLO_SECRETS_KEYFILE")
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let mut v = serde_json::Value::Null;
    while start.elapsed() < Duration::from_secs(10) {
        v = status(dir);
        if v["serving"].as_bool() == Some(true) || v.get("error").is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    v
}

#[test]
fn the_login_password_opens_the_store() {
    let dir = PathBuf::from(format!("/var/tmp/olo-secrets-login-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(
        dir.join("bus.conf"),
        format!(
            "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\" \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n<busconfig><type>session</type><listen>unix:path={}/bus</listen><policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/><allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>\n",
            dir.display()
        ),
    )
    .unwrap();
    let Ok(mut daemon) = Command::new("dbus-daemon").arg(format!("--config-file={}", dir.join("bus.conf").display())).arg("--nofork").stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        eprintln!("no dbus-daemon here");
        return;
    };
    let start = Instant::now();
    while !dir.join("bus").exists() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let bus = format!("unix:path={}/bus", dir.display());
    let (password, changed) = ([5u8; 32], [6u8; 32]);
    // The first login: a new store, its key sealed by the password.
    let first = run(&dir, &bus, &password);
    assert_eq!(first["serving"], true, "{first}");
    let sealed = std::fs::read(dir.join("secrets.login")).expect("sealed by the login password");
    assert!(dir.join("secrets.store").exists());
    // The next, the same password: opened, nothing asked.
    let next = run(&dir, &bus, &password);
    assert_eq!(next["serving"], true, "{next}");
    assert_eq!(std::fs::read(dir.join("secrets.login")).unwrap(), sealed, "sealed once");
    // Another password (changed since): no one to ask here, so not open,
    // and the seal kept for the old one.
    let other = run(&dir, &bus, &changed);
    assert_ne!(other["serving"], true, "{other}");
    assert!(other["error"].as_str().is_some_and(|e| e.contains("password")), "{other}");
    assert_eq!(std::fs::read(dir.join("secrets.login")).unwrap(), sealed);
    let _ = daemon.kill();
    let _ = daemon.wait();
    std::fs::remove_dir_all(&dir).unwrap();
}
