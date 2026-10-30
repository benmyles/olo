//! A Chrome already running with the profile folder is handed the
//! command line (a link to open, `--new-window`) directly, as a second
//! Chrome would hand it over, instead of starting one only for that: a
//! second Chrome says "Opening in existing browser session." and, as it
//! goes, its zygote complains of a broken pipe, on the terminal of
//! whatever opened the link (`xdg-open` from a shell, a CLI's sign-in),
//! and it takes a while to start. Chromium's process singleton on Linux:
//! the running Chrome listens on `SingletonSocket` in the profile folder
//! (`SingletonLock` names it, `host-pid`); a client writes `START`, its
//! folder and its arguments, each after a NUL, closes its side and is
//! answered `ACK` (or `SHUTDOWN`: that Chrome is on its way out, so start
//! another). An activation token goes along as `--xdg-activation-token`,
//! as Chrome's own hand-over passes it, so the window comes forward.

use std::{
    ffi::OsString,
    io::{Read, Write},
    os::unix::{ffi::OsStrExt, net::UnixStream},
    path::Path,
    time::Duration,
};

/// How long the running Chrome has to answer (Chrome's own wait).
const PATIENCE: Duration = Duration::from_secs(20);

/// The pid of the Chrome that has profile folder `dir` open on this
/// computer, if one is running.
fn holder(dir: &Path) -> Option<u32> {
    let lock = std::fs::read_link(dir.join("SingletonLock")).ok()?;
    let (host, pid) = lock.to_str()?.rsplit_once('-')?;
    let pid: u32 = pid.parse().ok()?;
    let ours = std::fs::read_to_string("/proc/sys/kernel/hostname").ok()?;
    (host == ours.trim() && Path::new(&format!("/proc/{pid}")).exists()).then_some(pid)
}

/// What a second Chrome writes: `START`, the folder it was started in,
/// then its arguments (its program first), each after a NUL.
fn message(cwd: &Path, argv: &[OsString]) -> Vec<u8> {
    let mut out = b"START".to_vec();
    out.push(0);
    out.extend_from_slice(cwd.as_os_str().as_bytes());
    for a in argv {
        out.push(0);
        out.extend_from_slice(a.as_bytes());
    }
    out
}

/// Hand `argv` (Chrome's program, then its arguments) to the Chrome
/// running with profile folder `dir`, with activation `token`. False if
/// none is, or it couldn't take it (then start Chrome as usual).
pub fn hand_over(dir: &Path, argv: &[OsString], token: Option<&str>) -> bool {
    if holder(dir).is_none() {
        return false;
    }
    let Ok(cwd) = std::env::current_dir() else { return false };
    let mut argv = argv.to_vec();
    if let Some(t) = token {
        argv.push(format!("--xdg-activation-token={t}").into());
    }
    match send(&dir.join("SingletonSocket"), &message(&cwd, &argv), PATIENCE) {
        Ok(answer) => answer.starts_with(b"ACK"),
        Err(e) => {
            tracing::debug!("couldn't hand the link to the running Chrome: {e}");
            false
        }
    }
}

/// Write `msg` to the socket at `path`, and read its answer.
fn send(path: &Path, msg: &[u8], patience: Duration) -> std::io::Result<Vec<u8>> {
    let mut s = UnixStream::connect(path)?;
    s.set_write_timeout(Some(patience))?;
    s.set_read_timeout(Some(patience))?;
    s.write_all(msg)?;
    s.shutdown(std::net::Shutdown::Write)?;
    let mut answer = Vec::new();
    s.take(64).read_to_end(&mut answer)?;
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_the_link_over() {
        let dir = std::env::temp_dir().join(format!("olo-chrome-singleton-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Nothing running: no.
        assert!(!hand_over(&dir, &["chrome".into()], None));
        // A Chrome (this process) holding the folder, and its socket.
        let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
        std::os::unix::fs::symlink(format!("{}-{}", host.trim(), std::process::id()), dir.join("SingletonLock")).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(dir.join("SingletonSocket")).unwrap();
        let chrome = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            s.read_to_end(&mut got).unwrap();
            s.write_all(b"ACK").unwrap();
            got
        });
        let argv: Vec<OsString> = ["/opt/chrome", "--ozone-platform=wayland", "https://example.com/login?a=1&b=2"].map(OsString::from).to_vec();
        assert!(hand_over(&dir, &argv, Some("tok-1")));
        let got = chrome.join().unwrap();
        let cwd = std::env::current_dir().unwrap();
        let want = format!("START\0{}\0/opt/chrome\0--ozone-platform=wayland\0https://example.com/login?a=1&b=2\0--xdg-activation-token=tok-1", cwd.display());
        assert_eq!(String::from_utf8(got).unwrap(), want);
        // One from another computer (a shared home), or gone: no.
        std::fs::remove_file(dir.join("SingletonLock")).unwrap();
        std::os::unix::fs::symlink("elsewhere-1", dir.join("SingletonLock")).unwrap();
        assert!(!hand_over(&dir, &argv, None));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
