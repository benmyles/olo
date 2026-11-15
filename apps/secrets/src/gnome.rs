//! GNOME Keyring, should something have started it (another login
//! screen's PAM, a person by hand): olo-secrets answers for the keyring,
//! so it asks GNOME Keyring to quit over its control socket
//! (gkd-control-server.c's GKD_CONTROL_OP_QUIT), which gives up
//! `org.freedesktop.secrets` to olo-secrets, waiting in line for it.
//! GNOME Keyring lets nobody replace it on the bus, so this is the way.
//! Nothing else of it is used: its items come over from its files, read
//! natively with the login password (`legacy.rs`), and nothing of Olo's
//! starts it or hands anything back to it (Ben, 2026-11-15: "we should
//! be 100% independent of gnome and not rely on any of its services").

use std::{
    io::{Read as _, Write as _},
    os::unix::{fs::FileTypeExt as _, fs::MetadataExt as _, net::UnixStream},
    path::PathBuf,
    time::Duration,
};

use zbus::blocking::Connection;

use crate::store::Item;

pub const NAME: &str = "org.freedesktop.secrets";

/// GKD_CONTROL_OP_QUIT and GKD_CONTROL_RESULT_OK.
const QUIT: u32 = 3;
const OK: u32 = 0;

/// One of GNOME Keyring's items, read from its files.
pub struct Found {
    /// Its keyring's id (its file's name) and label.
    pub collection: String,
    pub collection_label: String,
    pub item: Item,
}

/// Whether one of GNOME Keyring's keyrings changed since `at` (seconds
/// since 1970; None: never copied, so any there is new).
pub fn changed_since(at: Option<u64>) -> bool {
    std::fs::read_dir(crate::legacy::folder()).into_iter().flatten().flatten().any(|e| {
        e.path().extension().is_some_and(|x| x == "keyring")
            && e.metadata().and_then(|m| m.modified()).is_ok_and(|t| at.is_none_or(|at| t.duration_since(std::time::UNIX_EPOCH).is_ok_and(|d| d.as_secs() > at)))
    })
}

/// Who answers for the keyring now (a unique name), if anyone.
pub fn owner(conn: &Connection) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(conn).ok()?;
    dbus.get_name_owner(NAME.try_into().ok()?).ok().map(|o| o.to_string())
}

/// GNOME Keyring's control socket.
fn control() -> PathBuf {
    std::env::var_os("GNOME_KEYRING_CONTROL").map(PathBuf::from).unwrap_or_else(|| olo_xdg::runtime_dir().join("keyring")).join("control")
}

/// Ask the daemon to quit, and wait (up to five seconds) until it has:
/// it keeps the socket open till it exits. Ok(false): no daemon there.
pub fn quit() -> std::io::Result<bool> {
    let path = control();
    // Like the daemon's own client: only our socket, never a link.
    let Ok(m) = std::fs::symlink_metadata(&path) else { return Ok(false) };
    // SAFETY: geteuid cannot fail.
    if !m.file_type().is_socket() || m.uid() != unsafe { libc::geteuid() } {
        return Ok(false);
    }
    let mut s = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => return Ok(false),
        Err(e) => return Err(e),
    };
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    // Credentials (one zero byte; the daemon checks SO_PEERCRED), then
    // the packet: its length, the op.
    let mut packet = vec![0u8];
    packet.extend(8u32.to_be_bytes());
    packet.extend(QUIT.to_be_bytes());
    s.write_all(&packet)?;
    let mut reply = [0u8; 8];
    s.read_exact(&mut reply)?;
    let result = u32::from_be_bytes(reply[4..8].try_into().unwrap());
    if result != OK {
        return Err(std::io::Error::other(format!("GNOME Keyring wouldn't quit (result {result})")));
    }
    // Till it's gone (the socket closed), or the timeout.
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    Ok(true)
}
