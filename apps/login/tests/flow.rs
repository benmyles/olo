//! A login from start to finish, unprivileged (`olo-login --daemon
//! --test DIR`): the daemon on its socket, a greeter (this test) asking
//! who and which sessions, a wrong password and the right one through a
//! worker's PAM conversation (a stand-in stack: never the real PAM),
//! the session started, run and ended.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use olo_greet::{Event, PromptKind, Request, Way};

fn me() -> String {
    // SAFETY: getpwuid's static entry, copied at once.
    unsafe { std::ffi::CStr::from_ptr((*libc::getpwuid(libc::getuid())).pw_name).to_string_lossy().into_owned() }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

struct Greeter {
    out: UnixStream,
    lines: BufReader<UnixStream>,
}

impl Greeter {
    fn ask(&mut self, r: &Request) {
        writeln!(self.out, "{}", serde_json::to_string(r).unwrap()).unwrap();
    }

    fn next(&mut self) -> Event {
        let mut line = String::new();
        self.lines.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    }
}

fn wait_for(path: &Path) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn a_login_from_start_to_finish() {
    let dir = PathBuf::from(format!("/var/tmp/olo-login-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let user = me();
    // The stand-in: pam_exec pipes the password to a script that knows it.
    let check = dir.join("check.sh");
    write(&check, "#!/bin/sh\nread -r p\n[ \"$p\" = \"open sesame\" ]\n");
    std::fs::set_permissions(&check, std::fs::Permissions::from_mode(0o755)).unwrap();
    write(
        &dir.join("pam/olo-login"),
        &format!("auth required pam_exec.so type=auth expose_authtok quiet {}\nauth required pam_permit.so\naccount required pam_permit.so\nsession required pam_permit.so\n", check.display()),
    );
    // Its people (this test's user) and a session that says it ran.
    let ran = dir.join("ran");
    write(&dir.join("root/etc/passwd"), &format!("{user}:x:{uid}:{uid}:The Tester:/var/tmp:/bin/sh\n", uid = unsafe { libc::getuid() }));
    write(
        &dir.join("root/usr/share/wayland-sessions/test.desktop"),
        &format!("[Desktop Entry]\nName=Test Desktop\nExec=/bin/sh -c \"echo $XDG_SESSION_DESKTOP > {}\"\nDesktopNames=Test\n", ran.display()),
    );
    let mut daemon = std::process::Command::new(env!("CARGO_BIN_EXE_olo-login")).args(["--daemon", "--test"]).arg(&dir).spawn().unwrap();
    assert!(wait_for(&dir.join("greeter.sock")), "the daemon's socket");
    let out = UnixStream::connect(dir.join("greeter.sock")).unwrap();
    let mut g = Greeter { lines: BufReader::new(out.try_clone().unwrap()), out };

    g.ask(&Request::Users);
    let Event::Users { users } = g.next() else { panic!("users") };
    assert_eq!(users.iter().map(|u| (u.name.clone(), u.full_name.clone())).collect::<Vec<_>>(), [(user.clone(), "The Tester".to_owned())]);
    g.ask(&Request::Sessions);
    let Event::Sessions { sessions } = g.next() else { panic!("sessions") };
    assert_eq!(sessions.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Test Desktop"]);

    // A wrong password: refused.
    g.ask(&Request::Begin { user: user.clone() });
    assert!(matches!(g.next(), Event::Prompt { by: Way::Password, kind: PromptKind::Secret, .. }));
    g.ask(&Request::Answer { text: Some("open sesame!".into()) });
    assert!(matches!(g.next(), Event::Failed { by: Way::Password, .. }));
    // The right one: in.
    g.ask(&Request::Begin { user: user.clone() });
    assert!(matches!(g.next(), Event::Prompt { by: Way::Password, kind: PromptKind::Secret, .. }));
    g.ask(&Request::Answer { text: Some("open sesame".into()) });
    assert_eq!(g.next(), Event::Authenticated { by: Way::Password });
    g.ask(&Request::Start { session: "test".into() });
    assert_eq!(g.next(), Event::Started);
    // The session ran (with its desktop's name) and ended.
    assert!(wait_for(&dir.join("started")), "started");
    assert!(wait_for(&dir.join("ended")), "ended");
    assert_eq!(std::fs::read_to_string(&ran).unwrap().trim(), "test");
    let _ = daemon.kill();
    let _ = daemon.wait();
    std::fs::remove_dir_all(&dir).unwrap();
}
