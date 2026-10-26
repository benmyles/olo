//! keyd, when it runs, remaps keyboards for every session on the machine
//! (it grabs them and types through a virtual one). app_server maps the
//! Mac keyboard itself (keymap.rs), so a real Tabula session asks keyd,
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
const KEYS: &[&str] = &[
    "leftmeta",
    "rightmeta",
    "capslock",
    "leftcontrol",
    "rightcontrol",
    "leftalt",
    "rightalt",
    "leftshift",
    "rightshift",
];

/// keyd's `struct ipc_message` (keyd.h).
const BIND: u32 = 2;
const SUCCESS: u32 = 0;
const DATA: usize = 4096;
const SIZE_AT: usize = (8 + DATA).next_multiple_of(std::mem::align_of::<usize>());
const MESSAGE: usize = (SIZE_AT + std::mem::size_of::<usize>()).next_multiple_of(std::mem::align_of::<usize>());

fn socket() -> PathBuf {
    std::env::var_os("TABULA_KEYD_SOCKET").map(PathBuf::from).unwrap_or_else(|| "/run/keyd.socket".into())
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
    {
        crate::log(format!("keyd: {e}"));
    }
}

#[cfg(test)]
mod tests {
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
