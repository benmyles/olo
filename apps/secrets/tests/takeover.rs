//! olo-secrets taking over from a real GNOME Keyring and handing back,
//! with libsecret's own client (`secret-tool`, an encrypted session as
//! Chrome's) asking. Needs a private bus with its own GNOME Keyring
//! (login keyring unlocked with "test"), never the session's:
//!
//!   eval "$(tools/dev/private-keyring start | grep ^export)"
//!   KEYRING_TEST_BUS=$DBUS_SESSION_BUS_ADDRESS cargo test -p olo-secrets --test takeover -- --ignored --nocapture
//!   tools/dev/private-keyring stop

use std::{
    io::{BufRead as _, BufReader, Write as _},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use olo_system::keyring_prompt::{Answer, Event, Prompter};

/// Prompts answered.
static PROMPTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The private keyring's directory (tools/dev/private-keyring's).
const DIR: &str = "/tmp/olo-dev/keyring";

fn tool(bus: &str, args: &[&str], input: Option<&str>) -> (bool, String) {
    let mut c = Command::new("secret-tool");
    c.args(args)
        .env_clear()
        .env("DBUS_SESSION_BUS_ADDRESS", bus)
        .env("HOME", format!("{DIR}/home"))
        .env("XDG_RUNTIME_DIR", format!("{DIR}/run"))
        .env("PATH", "/usr/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("secret-tool");
    if let Some(i) = input {
        child.stdin.take().unwrap().write_all(i.as_bytes()).unwrap();
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
}

/// The private GNOME Keyring's daemons (by their environment).
fn keyring_daemons() -> Vec<u32> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if !comm.starts_with("gnome-keyring") {
            continue;
        }
        let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        if env.split(|b| *b == 0).any(|kv| kv == format!("XDG_RUNTIME_DIR={DIR}/run").as_bytes()) {
            out.push(pid);
        }
    }
    out
}

struct Secrets {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Secrets {
    fn start(bus: &str, scratch: &std::path::Path) -> Secrets {
        let mut child = Command::new(env!("CARGO_BIN_EXE_olo-secrets"))
            .env_clear()
            .env("DBUS_SESSION_BUS_ADDRESS", bus)
            .env("HOME", format!("{DIR}/home"))
            .env("XDG_RUNTIME_DIR", format!("{DIR}/run"))
            .env("XDG_CONFIG_HOME", scratch.join("config"))
            .env("OLO_SECRETS_SERVE", "stdin")
            .env("OLO_SECRETS_TEST_PASSWORD", "test")
            .env("OLO_SECRETS_STORE", scratch.join("secrets.store"))
            .env("RUST_LOG", "olo_secrets=debug,info")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("olo-secrets");
        let stdin = child.stdin.take().unwrap();
        let err = child.stderr.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(err).lines().map_while(Result::ok) {
                eprintln!("  olo-secrets: {l}");
                let _ = tx.send(l);
            }
        });
        Secrets { child, stdin, lines }
    }

    fn say(&mut self, what: &str) {
        writeln!(self.stdin, "{what}").unwrap();
    }

    /// Wait for a line saying `what`.
    fn wait_for(&self, what: &str) -> String {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(l) if l.contains(what) => return l,
                Ok(l) if l.contains("WARN") => panic!("olo-secrets: {l}"),
                Ok(_) => {}
                Err(_) => panic!("olo-secrets never said “{what}”"),
            }
        }
    }
}

impl Drop for Secrets {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore]
fn takes_over_and_hands_back() {
    let bus = std::env::var("KEYRING_TEST_BUS").expect("KEYRING_TEST_BUS (see the top)");
    assert!(bus.contains("/olo-dev/"), "not a private test bus: {bus}");
    // SAFETY: set before any thread reads them.
    unsafe {
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus);
        std::env::set_var("XDG_RUNTIME_DIR", format!("{DIR}/run"));
        std::env::remove_var("WAYLAND_DISPLAY");
    }
    let scratch = std::path::PathBuf::from(format!("/var/tmp/olo-secrets-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    // A prompter answering "test" (the private login keyring's password,
    // and the account's? no: GNOME Keyring's own unlock asks here).
    let (tx, rx) = mpsc::channel();
    let prompter = Arc::new(Prompter::serve(move |e| tx.send(e).unwrap()).unwrap());
    {
        let p = prompter.clone();
        std::thread::spawn(move || {
            for e in rx {
                if let Event::Show { id, .. } = e {
                    PROMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    eprintln!("  (prompt {id} answered)");
                    p.answer(id, Answer::Password("test".into()), false);
                }
            }
        });
    }

    // Items in GNOME Keyring first, as Chrome and a program of Olo's put them.
    assert!(tool(&bus, &["store", "--label=Chrome Safe Storage", "application", "chrome"], Some("chrome-key")).0);
    assert!(tool(&bus, &["store", "--label=Old", "application", "olo-test", "kind", "old"], Some("old-value")).0);
    assert_eq!(keyring_daemons().len(), 1, "one private GNOME Keyring");

    let mut s = Secrets::start(&bus, &scratch);
    s.say("on");
    let said = s.wait_for("answering for the keyring");
    // (Chrome's, the old one, and the store's own key.)
    assert!(said.contains("3 copied"), "{said}");
    // GNOME Keyring is gone; Olo answers.
    let end = Instant::now() + Duration::from_secs(5);
    while !keyring_daemons().is_empty() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(keyring_daemons().is_empty(), "GNOME Keyring quit");

    // libsecret's encrypted sessions, many times (a shared secret with a
    // zero first byte comes about once in 256).
    let t = Instant::now();
    for _ in 0..300 {
        let (ok, out) = tool(&bus, &["lookup", "application", "chrome"], None);
        assert!(ok && out == "chrome-key", "lookup: {out}");
    }
    eprintln!("300 lookups in {:?}", t.elapsed());
    // Stored, searched, replaced, cleared.
    assert!(tool(&bus, &["store", "--label=New", "application", "olo-test", "kind", "new"], Some("new-value")).0);
    assert_eq!(tool(&bus, &["lookup", "application", "olo-test", "kind", "new"], None).1, "new-value");
    assert!(tool(&bus, &["store", "--label=New", "application", "olo-test", "kind", "new"], Some("newer")).0);
    assert_eq!(tool(&bus, &["lookup", "application", "olo-test", "kind", "new"], None).1, "newer");
    let (_, found) = tool(&bus, &["search", "--all", "application", "olo-test"], None);
    assert!(found.contains("label = New") && found.contains("label = Old"), "{found}");
    assert!(tool(&bus, &["clear", "application", "olo-test", "kind", "old"], None).0);
    assert!(!tool(&bus, &["lookup", "application", "olo-test", "kind", "old"], None).0);
    // Locked, then unlocked by libsecret through a prompt (answered by
    // the prompter above with the test's password).
    let answered = PROMPTS.load(std::sync::atomic::Ordering::SeqCst);
    assert!(tool(&bus, &["lock", "--collection=/org/freedesktop/secrets/collection/login"], None).0);
    let (_, found) = tool(&bus, &["search", "application", "chrome"], None);
    assert!(!found.contains("chrome-key"), "locked: no secret given: {found}");
    assert_eq!(tool(&bus, &["lookup", "application", "chrome"], None).1, "chrome-key");
    assert_eq!(PROMPTS.load(std::sync::atomic::Ordering::SeqCst), answered + 1, "unlocking asked once");
    let status: serde_json::Value = serde_json::from_slice(&std::fs::read(format!("{DIR}/run/olo-secrets.json")).unwrap()).unwrap();
    assert_eq!(status["serving"], true);
    assert!(status["programs"].as_array().unwrap().iter().any(|p| p == "chrome"), "{status}");

    // Handed back: the new one written into GNOME Keyring (started again,
    // unlocked through the prompter), the deleted one deleted there.
    s.say("off");
    s.wait_for("handed back to GNOME Keyring (1 written, 1 deleted");
    assert_eq!(keyring_daemons().len(), 1, "GNOME Keyring again");
    assert_eq!(tool(&bus, &["lookup", "application", "olo-test", "kind", "new"], None).1, "newer");
    assert!(!tool(&bus, &["lookup", "application", "olo-test", "kind", "old"], None).0);
    assert_eq!(tool(&bus, &["lookup", "application", "chrome"], None).1, "chrome-key");

    // And over again: only what changed in GNOME Keyring since comes.
    assert!(tool(&bus, &["store", "--label=Chrome Safe Storage", "application", "chrome"], Some("chrome-key-2")).0);
    s.say("on");
    let said = s.wait_for("answering for the keyring");
    assert!(said.contains("2 copied"), "the changed one and the one handed back: {said}");
    assert_eq!(tool(&bus, &["lookup", "application", "chrome"], None).1, "chrome-key-2");

    // Started again (an upgrade): no GNOME Keyring to ask, nothing asked.
    drop(s);
    let prompts = PROMPTS.load(std::sync::atomic::Ordering::SeqCst);
    let mut s = Secrets::start(&bus, &scratch);
    s.say("on");
    let said = s.wait_for("answering for the keyring");
    assert!(said.contains("0 copied"), "{said}");
    assert!(keyring_daemons().is_empty());
    assert_eq!(tool(&bus, &["lookup", "application", "chrome"], None).1, "chrome-key-2");
    assert_eq!(PROMPTS.load(std::sync::atomic::Ordering::SeqCst), prompts, "nothing asked");
    // Handed back at the end (the key kept for the login forgotten).
    s.say("off");
    s.wait_for("handed back to GNOME Keyring");
    drop(s);
    let _ = std::fs::remove_dir_all(&scratch);
}
