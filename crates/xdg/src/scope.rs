//! Programs Olo starts, each in a systemd scope of its own named for it,
//! `app-olo-<app id>-<pid>.scope` (systemd's convention for desktops,
//! <https://systemd.io/DESKTOP_ENVIRONMENTS/>), so the portals can tell
//! which program is asking: xdg-desktop-portal reads an unsandboxed
//! program's app id from its unit's name (the desktop entry's id), for
//! screen sharing, Inhibit, GlobalShortcuts and notifications. Asked of
//! the person's systemd over D-Bus (`StartTransientUnit`, with the
//! process's pid) just after it starts, on a thread of its own; where
//! there is no systemd (a test session's bus) it stays where it was.
//! Chromium and Electron programs move themselves into a scope of their
//! own (`app-com.google.Chrome-….scope`) as they start, which wins.
//! Olo's own programs aren't put in one: they belong to the session.

use std::time::Duration;

use zbus::zvariant::Value;

/// How long systemd has to answer.
const PATIENCE: Duration = Duration::from_secs(2);

/// `s` as part of a unit name: systemd's escaping, where `-` would
/// otherwise split it (`google-chrome` is `google\x2dchrome`).
fn escape(s: &str) -> String {
    let mut out = String::new();
    for (i, b) in s.bytes().enumerate() {
        if b.is_ascii_alphanumeric() || b == b':' || b == b'_' || (b == b'.' && i > 0) {
            out.push(b as char);
        } else {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
}

/// The scope's name for `app_id`'s process `pid`.
pub fn unit_name(app_id: &str, pid: u32) -> String {
    format!("app-olo-{}-{pid}.scope", escape(app_id))
}

/// Whether `program` (a command's first word) is one of Olo's own.
pub fn ours(program: &std::ffi::OsStr) -> bool {
    std::path::Path::new(program).file_name().is_some_and(|n| n.as_encoded_bytes().starts_with(b"olo-"))
}

/// Put process `pid`, just started for desktop entry `app_id`, in a
/// scope of its own (in the background: systemd's answer isn't waited
/// for).
pub fn place(pid: u32, app_id: &str) {
    if app_id.is_empty() {
        return;
    }
    let name = unit_name(app_id, pid);
    let _ = std::thread::Builder::new().name("app-scope".into()).spawn(move || {
        let started = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.method_timeout(PATIENCE).build())
            .and_then(|bus| start(&bus, &name, pid));
        match started {
            Ok(()) => tracing::debug!("started {name}"),
            Err(e) => tracing::debug!("{name}: {e}"),
        }
    });
}

/// Ask systemd's manager on `bus` for scope `name` holding `pid`.
fn start(bus: &zbus::blocking::Connection, name: &str, pid: u32) -> zbus::Result<()> {
    let properties: Vec<(&str, Value)> =
        vec![("PIDs", Value::from(vec![pid])), ("CollectMode", Value::from("inactive-or-failed")), ("Description", Value::from("Application started by Olo"))];
    let aux: Vec<(&str, Vec<(&str, Value)>)> = Vec::new();
    bus.call_method(
        Some("org.freedesktop.systemd1"),
        "/org/freedesktop/systemd1",
        Some("org.freedesktop.systemd1.Manager"),
        "StartTransientUnit",
        &(name, "fail", properties, aux),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app id xdg-desktop-portal reads from a scope's name (its
    /// pattern, `^app-(?:[[:alnum:]]+\-)?(.+?)(?:\-[[:alnum:]]*)\.scope$`,
    /// then systemd's unescaping), for names with a launcher.
    fn portal_reads(unit: &str) -> String {
        let rest = unit.strip_prefix("app-").unwrap().strip_suffix(".scope").unwrap();
        let (_launcher, rest) = rest.split_once('-').unwrap();
        let (escaped, random) = rest.split_once('-').unwrap();
        assert!(random.chars().all(|c| c.is_ascii_alphanumeric()), "{unit}");
        let mut out = Vec::new();
        let b = escaped.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' && b.get(i + 1) == Some(&b'x') {
                out.push(u8::from_str_radix(&escaped[i + 2..i + 4], 16).unwrap());
                i += 4;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn names_the_scope_for_the_portal() {
        assert_eq!(unit_name("google-chrome", 42), "app-olo-google\\x2dchrome-42.scope");
        assert_eq!(unit_name("org.gnome.Nautilus", 7), "app-olo-org.gnome.Nautilus-7.scope");
        for id in ["slack", "google-chrome", "org.gnome.Evolution-alarm-notify", "dev.zed.Zed", "a b+c"] {
            assert_eq!(portal_reads(&unit_name(id, 1234)), id);
        }
        assert!(ours("/home/ann/.local/bin/olo-chrome".as_ref()) && ours("olo-terminal".as_ref()));
        assert!(!ours("/usr/bin/gedit".as_ref()));
    }

    /// systemd's manager, as far as starting a scope goes.
    struct Manager(std::sync::mpsc::Sender<(String, String, Vec<u32>, String)>);

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl Manager {
        fn start_transient_unit(
            &self,
            name: String,
            mode: String,
            properties: Vec<(String, zbus::zvariant::OwnedValue)>,
            _aux: Vec<(String, Vec<(String, zbus::zvariant::OwnedValue)>)>,
        ) -> zbus::zvariant::OwnedObjectPath {
            let get = |k: &str| properties.iter().find(|(n, _)| n == k).map(|(_, v)| v.try_clone().unwrap());
            let pids: Vec<u32> = get("PIDs").and_then(|v| v.try_into().ok()).unwrap_or_default();
            let collect: String = get("CollectMode").and_then(|v| v.try_into().ok()).unwrap_or_default();
            let _ = self.0.send((name, mode, pids, collect));
            zbus::zvariant::OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/1").unwrap()
        }
    }

    #[test]
    fn asks_systemd_for_the_scope() {
        // A stand-in for systemd at the other end of a private connection.
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let guid = zbus::Guid::generate();
        let server = std::thread::spawn(move || {
            zbus::blocking::connection::Builder::async_io_unix_stream(a)
                .server(guid)
                .unwrap()
                .p2p()
                .serve_at("/org/freedesktop/systemd1", Manager(tx))
                .unwrap()
                .build()
                .unwrap()
        });
        let bus = zbus::blocking::connection::Builder::async_io_unix_stream(b).p2p().method_timeout(PATIENCE).build().unwrap();
        let _server = server.join().unwrap();
        start(&bus, &unit_name("dev.zed.Zed", 4321), 4321).unwrap();
        let (name, mode, pids, collect) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((name.as_str(), mode.as_str(), pids, collect.as_str()), ("app-olo-dev.zed.Zed-4321.scope", "fail", vec![4321], "inactive-or-failed"));
    }

    /// The person's own systemd, with a throwaway process: its scope is
    /// gone when the process is. Only when asked for (`--ignored`).
    #[test]
    #[ignore = "starts a scope in the session's systemd"]
    fn a_real_scope() {
        let mut child = std::process::Command::new("/bin/sleep").arg("3").spawn().unwrap();
        let pid = child.id();
        let bus = zbus::blocking::connection::Builder::session().unwrap().method_timeout(PATIENCE).build().unwrap();
        start(&bus, &unit_name("org.example.Olo-Scope-Test", pid), pid).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap();
        println!("{cgroup}");
        assert!(cgroup.trim_end().ends_with(&format!("/app-olo-org.example.Olo\\x2dScope\\x2dTest-{pid}.scope")), "{cgroup}");
        child.wait().unwrap();
    }
}
