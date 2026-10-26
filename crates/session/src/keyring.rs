//! The login keyring. At login, PAM (pam_gnome_keyring) starts
//! gnome-keyring-daemon and unlocks the login keyring with the password
//! just typed, then the daemon waits for the desktop to finish starting it
//! over its control socket. Unless that happens within two minutes it
//! quits, and the next program to want a secret gets a fresh daemon that
//! has to ask for the password again. GNOME finishes the start with
//! `gnome-keyring-daemon --start`; a real Tabula session speaks the same
//! control protocol (daemon/control in gnome-keyring) itself.

use std::{
    io::{Read, Write},
    os::unix::{fs::FileTypeExt, fs::MetadataExt, net::UnixStream},
    path::{Path, PathBuf},
    time::Duration,
};

/// GKD_CONTROL_OP_INITIALIZE and GKD_CONTROL_RESULT_OK.
const INITIALIZE: u32 = 0;
const OK: u32 = 0;

/// What the daemon serves once started (as GNOME's autostart entry asks).
const COMPONENTS: &str = "secrets";

/// The variables the daemon needs to reach the session (its D-Bus name,
/// and prompts on our display).
const PASS: &[&str] = &["DBUS_SESSION_BUS_ADDRESS", "WAYLAND_DISPLAY", "DISPLAY", "XDG_RUNTIME_DIR", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE", "LANG"];

fn control() -> Option<PathBuf> {
    let dir = std::env::var_os("GNOME_KEYRING_CONTROL").map(PathBuf::from).or_else(|| Some(crate::runtime_dir().join("keyring")))?;
    Some(dir.join("control"))
}

/// Finish starting the daemon PAM started, if there is one, and take on
/// the environment it hands back (GNOME_KEYRING_CONTROL, SSH_AUTH_SOCK…).
pub fn start() {
    let Some(path) = control() else { return };
    let env: Vec<String> = PASS.iter().filter_map(|k| std::env::var(k).ok().map(|v| format!("{k}={v}"))).collect();
    match initialize(&path, &env) {
        Ok(Some(back)) => {
            for kv in back {
                if let Some((k, v)) = kv.split_once('=')
                    && !k.is_empty()
                {
                    // SAFETY: single-threaded start-up.
                    unsafe { std::env::set_var(k, v) };
                }
            }
            crate::log("login keyring started");
        }
        Ok(None) => {}
        Err(e) => crate::log(format!("keyring: {e}")),
    }
}

/// Ok(None): no daemon waiting there.
fn initialize(path: &Path, env: &[String]) -> std::io::Result<Option<Vec<String>>> {
    // Like the daemon's own client: only our socket, never a link.
    let Ok(m) = std::fs::symlink_metadata(path) else { return Ok(None) };
    // SAFETY: geteuid cannot fail.
    if !m.file_type().is_socket() || m.uid() != unsafe { libc::geteuid() } {
        return Ok(None);
    }
    let mut s = match UnixStream::connect(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => return Ok(None),
        Err(e) => return Err(e),
    };
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    // Credentials: one zero byte (the daemon checks SO_PEERCRED).
    s.write_all(&[0])?;
    s.write_all(&request(env))?;
    let mut len = [0u8; 4];
    s.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if !(8..1 << 20).contains(&len) {
        return Err(std::io::Error::other("bad reply"));
    }
    let mut body = vec![0u8; len - 4];
    s.read_exact(&mut body)?;
    reply(&body).map(Some)
}

/// An INITIALIZE packet: big-endian length (of the whole packet), op,
/// components, then the environment as a counted list of strings.
fn request(env: &[String]) -> Vec<u8> {
    let mut b = vec![0u8; 4];
    b.extend(INITIALIZE.to_be_bytes());
    put_str(&mut b, COMPONENTS);
    b.extend((env.len() as u32).to_be_bytes());
    for e in env {
        put_str(&mut b, e);
    }
    let n = b.len() as u32;
    b[0..4].copy_from_slice(&n.to_be_bytes());
    b
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    b.extend((s.len() as u32).to_be_bytes());
    b.extend(s.as_bytes());
}

/// The reply after its length: a result, then (if OK) the environment.
fn reply(body: &[u8]) -> std::io::Result<Vec<String>> {
    let bad = || std::io::Error::other("bad reply");
    let mut at = 0;
    let u32_at = |at: &mut usize| -> std::io::Result<u32> {
        let v = body.get(*at..*at + 4).ok_or_else(bad)?;
        *at += 4;
        Ok(u32::from_be_bytes(v.try_into().unwrap()))
    };
    let result = u32_at(&mut at)?;
    if result != OK {
        return Err(std::io::Error::other(format!("the daemon refused (result {result})")));
    }
    let n = u32_at(&mut at)?;
    let mut out = Vec::new();
    for _ in 0..n {
        let len = u32_at(&mut at)?;
        if len == u32::MAX {
            continue;
        }
        let s = body.get(at..at + len as usize).ok_or_else(bad)?;
        at += len as usize;
        out.push(String::from_utf8_lossy(s).into_owned());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    /// A stand-in daemon answering like gkd-control-server.c.
    #[test]
    fn starts_the_daemon_pam_left() {
        let dir = std::env::temp_dir().join(format!("tabula-keyring-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("control");
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut c, _) = listener.accept().unwrap();
            let mut cred = [9u8];
            c.read_exact(&mut cred).unwrap();
            let mut len = [0u8; 4];
            c.read_exact(&mut len).unwrap();
            let mut body = vec![0u8; u32::from_be_bytes(len) as usize - 4];
            c.read_exact(&mut body).unwrap();
            let mut reply = vec![0u8; 4];
            reply.extend(0u32.to_be_bytes());
            reply.extend(2u32.to_be_bytes());
            for s in ["GNOME_KEYRING_CONTROL=/k", "SSH_AUTH_SOCK=/k/ssh"] {
                reply.extend((s.len() as u32).to_be_bytes());
                reply.extend(s.as_bytes());
            }
            let n = reply.len() as u32;
            reply[0..4].copy_from_slice(&n.to_be_bytes());
            c.write_all(&reply).unwrap();
            (cred[0], body)
        });
        let env = vec!["WAYLAND_DISPLAY=wayland-1".to_string()];
        let back = super::initialize(&path, &env).unwrap().unwrap();
        let (cred, body) = server.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(cred, 0);
        assert_eq!(&body[0..4], &0u32.to_be_bytes());
        assert_eq!(&body[4..8], &7u32.to_be_bytes());
        assert_eq!(&body[8..15], b"secrets");
        assert_eq!(&body[15..19], &1u32.to_be_bytes());
        assert_eq!(&body[19..23], &25u32.to_be_bytes());
        assert_eq!(&body[23..], b"WAYLAND_DISPLAY=wayland-1");
        assert_eq!(back, vec!["GNOME_KEYRING_CONTROL=/k", "SSH_AUTH_SOCK=/k/ssh"]);
        assert_eq!(super::initialize(&dir.join("none"), &env).unwrap(), None);
    }
}

#[cfg(test)]
mod live {
    /// Against a real daemon (run by hand: see the commit that added it).
    #[test]
    #[ignore]
    fn real_daemon() {
        let dir = std::env::var("KEYRING_TEST_DIR").unwrap();
        let back = super::initialize(&std::path::Path::new(&dir).join("control"), &["LANG=C".into()]).unwrap();
        eprintln!("{back:?}");
        assert!(back.is_some());
    }
}
