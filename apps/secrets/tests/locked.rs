//! Locked, never in silence, and the move from GNOME Keyring with nothing
//! of GNOME's running (Ben, 2026-11-15: his keyring stayed locked after a
//! login by face, and nothing said so). On a private bus with Olo's
//! prompter answered by this test and a notification service of its own:
//!
//! 1. A store whose key an earlier version kept in GNOME Keyring's login
//!    keyring (the fixture, made by GNOME Keyring 51.1), not sealed by
//!    the login password, and no key from PAM: the password is asked
//!    for. Not Now: locked, and a notification says so.
//! 2. A program asking for a secret through the stand-in a moment later
//!    isn't asked again (no prompt), and goes unanswered.
//! 3. The notification's Unlock asks again; the password typed opens
//!    GNOME's file natively, the key in it opens the store, and it's
//!    sealed by the password. The notification goes.
//! 4. `Unlock` over the bus says it's open.
//! 5. The next login, with the key PAM gives for that password, opens it
//!    without asking.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use olo_system::keyring_prompt::{Answer, Event, Prompter};

const PASSWORD: &str = "fixture password";
/// The store's key as the fixture keeps it (an earlier olo-secrets put it
/// in GNOME Keyring's login keyring).
const KEY: &[u8; 32] = b"0123456789abcdef0123456789abcdef";

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

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(start.elapsed() < Duration::from_secs(15), "waited 15 s for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn olo_secrets(dir: &Path, bus: &str, args: &[&str], extra: &[(&str, &Path)]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_olo-secrets"));
    c.args(args)
        .env("DBUS_SESSION_BUS_ADDRESS", bus)
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("OLO_SECRETS_SERVE", "1")
        .env("OLO_SECRETS_STORE", dir.join("secrets.store"))
        .env("OLO_SECRETS_TEST_PASSWORD", PASSWORD)
        .env_remove("OLO_SECRETS_KEYFILE")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("OLO_SOCKET")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in extra {
        c.env(k, v);
    }
    c
}

/// What the notification service was told: (summary, its id) shown, and
/// the ids closed.
#[derive(Default)]
struct Told {
    shown: Vec<(String, u32)>,
    closed: Vec<u32>,
}

struct Notifications {
    told: Arc<Mutex<Told>>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(&self, _app: String, replaces: u32, _icon: String, summary: String, _body: String, _actions: Vec<String>, _hints: HashMap<String, zbus::zvariant::OwnedValue>, _timeout: i32) -> u32 {
        let id = if replaces == 0 { 7 } else { replaces };
        self.told.lock().unwrap().shown.push((summary, id));
        id
    }

    fn close_notification(&self, id: u32) {
        self.told.lock().unwrap().closed.push(id);
    }

    #[zbus(signal)]
    async fn action_invoked(emitter: &zbus::object_server::SignalEmitter<'_>, id: u32, key: &str) -> zbus::Result<()>;
}

#[test]
fn locked_is_said_and_unlocked_and_moved_from_gnome_natively() {
    let dir = PathBuf::from(format!("/var/tmp/olo-secrets-locked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::create_dir_all(dir.join("data/keyrings")).unwrap();
    std::fs::write(dir.join("data/keyrings/login.keyring"), include_bytes!("data/gnome-login.keyring")).unwrap();
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
    wait_for("the bus", || dir.join("bus").exists());
    let bus = format!("unix:path={}/bus", dir.display());
    // (This test's own prompter and notifications are on that bus: the
    // only test in this program, so nothing else reads the variable.)
    // SAFETY: set before any thread of this test uses the environment.
    unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus) };

    // An earlier version's store, its key the one in GNOME's file.
    let keyfile = dir.join("keyfile");
    std::fs::write(&keyfile, KEY).unwrap();
    let mut first = olo_secrets(&dir, &bus, &[], &[("OLO_SECRETS_KEYFILE", &keyfile)]).spawn().unwrap();
    wait_for("the earlier version's store", || status(&dir)["serving"] == true);
    let _ = first.kill();
    let _ = first.wait();
    std::fs::remove_file(&keyfile).unwrap();
    assert!(dir.join("secrets.store").exists() && !dir.join("secrets.login").exists());

    let told = Arc::new(Mutex::new(Told::default()));
    let notes = zbus::blocking::connection::Builder::address(bus.as_str())
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at("/org/freedesktop/Notifications", Notifications { told: told.clone() })
        .unwrap()
        .build()
        .unwrap();
    let (shown_tx, shown) = mpsc::channel();
    let prompter = Prompter::serve(move |e| {
        if let Event::Show { id, props, .. } = e {
            let _ = shown_tx.send((id, props.description));
        }
    })
    .unwrap();

    // 1. Not sealed, no key from PAM: asked; Not Now.
    join_new_session_keyring();
    let mut olo: Child = olo_secrets(&dir, &bus, &[], &[]).spawn().unwrap();
    let (id, description) = shown.recv_timeout(Duration::from_secs(15)).expect("the password asked for");
    assert!(description.contains("GNOME Keyring"), "{description}");
    prompter.answer(id, Answer::Cancel, false);
    wait_for("locked", || status(&dir)["locked"] == true);
    wait_for("the notification", || told.lock().unwrap().shown.iter().any(|(s, _)| s.contains("locked")));

    // 2. A program's request a moment later: not asked again.
    let started = Instant::now();
    let stand_in = olo_secrets(&dir, &bus, &["--activated"], &[]).status().unwrap();
    assert!(!stand_in.success());
    assert!(started.elapsed() < Duration::from_secs(5), "the stand-in waited {:?}", started.elapsed());
    assert!(shown.try_recv().is_err(), "asked again right after Not Now");

    // 3. The notification's Unlock: asked, opened from GNOME's file,
    // sealed; the notification taken away.
    let note_id = told.lock().unwrap().shown.last().unwrap().1;
    zbus::block_on(notes.inner().emit_signal(None::<()>, "/org/freedesktop/Notifications", "org.freedesktop.Notifications", "ActionInvoked", &(note_id, "unlock"))).unwrap();
    let (id, _) = shown.recv_timeout(Duration::from_secs(15)).expect("asked again by Unlock");
    prompter.answer(id, Answer::Password("not it".into()), false);
    let (id, _) = shown.recv_timeout(Duration::from_secs(15)).expect("asked again after a wrong password");
    prompter.answer(id, Answer::Password(PASSWORD.into()), false);
    wait_for("serving", || status(&dir)["serving"] == true);
    let s = status(&dir);
    assert!(s.get("locked").is_none() && s.get("error").is_none(), "{s}");
    assert!(dir.join("secrets.login").exists(), "sealed by the login password");
    wait_for("the notification closed", || told.lock().unwrap().closed.contains(&note_id));

    // 4. Unlock over the bus: open, nothing asked.
    let conn = zbus::blocking::connection::Builder::address(bus.as_str()).unwrap().build().unwrap();
    let p = zbus::blocking::Proxy::new(&conn, "org.olo.Secrets1", "/org/olo/Secrets1", "org.olo.Secrets1").unwrap();
    let open: bool = p.call("Unlock", &(false,)).unwrap();
    assert!(open);
    assert!(shown.try_recv().is_err());
    let _ = olo.kill();
    let _ = olo.wait();

    // 5. The next login: PAM's key for that password opens it, unasked.
    join_new_session_keyring();
    let uid = unsafe { libc::getuid() };
    give_login_key(&olo_login_key::login_key(PASSWORD.as_bytes(), uid, &olo_login_key::machine_id()));
    let _ = std::fs::remove_file(dir.join("run/olo-secrets.json"));
    let mut next = olo_secrets(&dir, &bus, &[], &[]).spawn().unwrap();
    wait_for("the next login's serving", || status(&dir)["serving"] == true || status(&dir).get("error").is_some());
    assert_eq!(status(&dir)["serving"], true, "{}", status(&dir));
    assert!(shown.try_recv().is_err(), "the next login asked");
    let _ = next.kill();
    let _ = next.wait();

    drop(prompter);
    let _ = daemon.kill();
    let _ = daemon.wait();
    std::fs::remove_dir_all(&dir).unwrap();
}
