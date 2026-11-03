//! keyd, when it runs, remaps keyboards for every session on the machine
//! (it grabs them and types through a virtual one). app_server maps the
//! Mac keyboard itself (keymap.rs), so a real Olo session asks keyd,
//! over its socket, to pass the modifier keys through as they are, and
//! puts keyd's own configuration back when it ends. Without keyd (or
//! without access to its socket) this does nothing.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

/// The keys app_server gives meaning to (keymap.rs), bound to themselves
/// in keyd's main layer.
const KEYS: &[&str] = &["leftmeta", "rightmeta", "capslock", "leftcontrol", "rightcontrol", "leftalt", "rightalt", "leftshift", "rightshift"];

/// keyd's `struct ipc_message` (keyd.h).
const BIND: u32 = 2;
const SUCCESS: u32 = 0;
const DATA: usize = 4096;
const SIZE_AT: usize = (8 + DATA).next_multiple_of(std::mem::align_of::<usize>());
const MESSAGE: usize = (SIZE_AT + std::mem::size_of::<usize>()).next_multiple_of(std::mem::align_of::<usize>());

fn socket() -> PathBuf {
    std::env::var_os("OLO_KEYD_SOCKET").map(PathBuf::from).unwrap_or_else(|| "/run/keyd.socket".into())
}

fn message(kind: u32, data: &str) -> Vec<u8> {
    let mut m = vec![0u8; MESSAGE];
    let data = &data.as_bytes()[..data.len().min(DATA - 1)];
    m[0..4].copy_from_slice(&kind.to_ne_bytes());
    m[8..8 + data.len()].copy_from_slice(data);
    m[SIZE_AT..SIZE_AT + std::mem::size_of::<usize>()].copy_from_slice(&data.len().to_ne_bytes());
    m
}

/// Send one bind expression; Ok(false) when keyd turned it down.
fn bind(expression: &str) -> std::io::Result<bool> {
    let mut s = UnixStream::connect(socket())?;
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    s.set_write_timeout(Some(Duration::from_secs(1)))?;
    s.write_all(&message(BIND, expression))?;
    let mut reply = vec![0u8; MESSAGE];
    s.read_exact(&mut reply)?;
    Ok(u32::from_ne_bytes(reply[0..4].try_into().unwrap()) == SUCCESS)
}

/// Is keyd there for us to talk to?
pub fn present() -> bool {
    socket().exists()
}

/// keyd isn't running: its socket is gone, or left behind by a keyd
/// that was stopped (or uninstalled, as Olo Doctor offers) and refuses.
fn gone(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound)
}

/// Have keyd pass app_server's keys through unchanged. (Repeated now and
/// then, since a `keyd reload` forgets binds.)
pub fn pass_through() {
    if !present() {
        return;
    }
    for key in KEYS {
        match bind(&format!("{key} = {key}")) {
            Ok(true) => {}
            Ok(false) => crate::log(format!("keyd would not pass {key} through")),
            Err(e) if gone(&e) => return,
            Err(e) => {
                // Once is enough (this repeats every minute).
                static SAID: std::sync::Once = std::sync::Once::new();
                SAID.call_once(|| crate::log(format!("keyd: {e}")));
                return;
            }
        }
    }
}

/// Put keyd's own configuration back.
pub fn restore() {
    if present()
        && let Err(e) = bind("reset")
        && !gone(&e)
    {
        crate::log(format!("keyd: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    /// A stand-in keyd: answers like keyd.c's IPC_BIND handler (success,
    /// or failure for "nope = nope") and records what it was sent.
    #[test]
    fn talks_to_keyd() {
        let path = std::env::temp_dir().join(format!("olo-keyd-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut got = Vec::new();
            for _ in 0..super::KEYS.len() + 1 {
                let (mut c, _) = listener.accept().unwrap();
                let mut m = vec![0u8; super::MESSAGE];
                c.read_exact(&mut m).unwrap();
                let kind = u32::from_ne_bytes(m[0..4].try_into().unwrap());
                let n = usize::from_ne_bytes(m[super::SIZE_AT..super::SIZE_AT + 8].try_into().unwrap());
                let text = String::from_utf8(m[8..8 + n].to_vec()).unwrap();
                let mut reply = vec![0u8; super::MESSAGE];
                reply[0..4].copy_from_slice(&(if text.starts_with("nope") { 1u32 } else { 0 }).to_ne_bytes());
                c.write_all(&reply).unwrap();
                got.push((kind, text));
            }
            got
        });
        // SAFETY: only this test reads the variable.
        unsafe { std::env::set_var("OLO_KEYD_SOCKET", &path) };
        super::pass_through();
        super::restore();
        let got = server.join().unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(got.iter().all(|(k, _)| *k == super::BIND));
        assert_eq!(got[0].1, "leftmeta = leftmeta");
        assert_eq!(got[2].1, "capslock = capslock");
        assert_eq!(got.last().unwrap().1, "reset");
        assert_eq!(got.len(), super::KEYS.len() + 1);
    }

    #[test]
    fn lays_out_like_keyd() {
        // On 64-bit Linux: u32 type, u32 timeout, char[4096], size_t.
        assert_eq!(super::MESSAGE, 4112);
        let m = super::message(super::BIND, "leftmeta = leftmeta");
        assert_eq!(&m[0..4], &2u32.to_ne_bytes());
        assert_eq!(&m[8..27], b"leftmeta = leftmeta");
        assert_eq!(&m[4104..4112], &19usize.to_ne_bytes());
    }
}
