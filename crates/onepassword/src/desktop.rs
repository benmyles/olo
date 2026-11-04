//! Talking to the 1Password desktop app ("Settings > Developer > Integrate
//! with other apps"), natively: what the official SDKs do through the
//! app's `libop_sdk_ipc_client`.
//!
//! The app listens on the abstract Unix socket `1PASSWORD_SDK_INTERGATIONS`
//! (sic). Each call is one connection: a request, then a response, each
//! JSON split into chunks of at most `CHUNK` bytes, every chunk framed as a
//! little-endian `u32` length (counting the flag byte), a flag byte (`MORE`
//! or `LAST`) and the bytes. Requests are `{kind, account_name, payload}`
//! and responses `{success, payload}`, payloads base64 (Go's `[]byte`;
//! newer apps answer with an array of bytes, which we read too). The
//! app authorises the caller itself (it asks the person, then remembers).

use std::{
    io::{self, Read, Write},
    os::{
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr, UnixStream},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{Error, types::base64_bytes};

/// Where the app listens.
pub const SOCKET: &str = "1PASSWORD_SDK_INTERGATIONS";
/// The most a chunk carries.
pub const CHUNK: usize = 499_999;
const MORE: u8 = 2;
const LAST: u8 = 1;
/// Responses bigger than this are refused (the SDKs' message limit, plus
/// room for base64).
const MAX_RESPONSE: usize = 80 * 1024 * 1024;
/// Long enough for the person to answer the app's authorisation prompt.
const TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Serialize)]
struct Request<'a> {
    kind: &'a str,
    account_name: &'a str,
    #[serde(with = "base64_bytes")]
    payload: Vec<u8>,
}

#[derive(Deserialize)]
struct Response {
    success: bool,
    #[serde(with = "base64_bytes", default)]
    payload: Vec<u8>,
}

/// A connection target: the app's socket, for one account.
#[derive(Clone, Debug)]
pub struct Desktop {
    pub account: String,
    pub socket: String,
}

/// Whether the 1Password desktop app is running (a process of this user's
/// whose program is 1Password's, `/opt/1Password/1password` or the
/// Flatpak's): told apart from its not letting apps in (its socket is
/// there only with Settings ▸ Developer ▸ Integrate with other apps on).
pub fn app_running() -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(me) = std::fs::metadata("/proc/self").map(|m| m.uid()) else { return false };
    std::fs::read_dir("/proc").into_iter().flatten().flatten().any(|e| {
        let dir = e.path();
        let ours = std::fs::metadata(&dir).is_ok_and(|m| m.uid() == me);
        ours && std::fs::read(dir.join("comm")).is_ok_and(|c| c.trim_ascii() == b"1password")
    })
}

impl Desktop {
    pub fn new(account: impl Into<String>) -> Self {
        Desktop { account: account.into(), socket: SOCKET.into() }
    }

    /// One call: `kind` is `init_client`, `invoke` or `release_client`.
    /// Returns the response payload; a failure's payload is the core's
    /// error (`{"name", "message"}` JSON).
    pub fn call(&self, kind: &str, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
        let request = serde_json::to_vec(&Request { kind, account_name: &self.account, payload })?;
        let addr = SocketAddr::from_abstract_name(self.socket.as_bytes()).map_err(Error::Io)?;
        let mut stream = UnixStream::connect_addr(&addr).map_err(|_| Error::DesktopAppUnavailable)?;
        stream.set_read_timeout(Some(TIMEOUT)).ok();
        stream.set_write_timeout(Some(TIMEOUT)).ok();
        write_message(&mut stream, &request).map_err(|_| Error::ConnectionDropped)?;
        let response = read_message(&mut stream).map_err(|e| match e.kind() {
            io::ErrorKind::InvalidData => Error::Io(e),
            _ => Error::ConnectionDropped,
        })?;
        let response: Response = serde_json::from_slice(&response)?;
        if response.success { Ok(response.payload) } else { Err(Error::from_core(&response.payload)) }
    }
}

/// Writes `message` as chunks.
pub fn write_message(w: &mut impl Write, message: &[u8]) -> io::Result<()> {
    let mut chunks = message.chunks(CHUNK).peekable();
    if message.is_empty() {
        return write_chunk(w, LAST, &[]);
    }
    while let Some(chunk) = chunks.next() {
        write_chunk(w, if chunks.peek().is_some() { MORE } else { LAST }, chunk)?;
    }
    w.flush()
}

fn write_chunk(w: &mut impl Write, flag: u8, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len() + 1).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut frame = Vec::with_capacity(bytes.len() + 5);
    frame.extend_from_slice(&len.to_le_bytes());
    frame.push(flag);
    frame.extend_from_slice(bytes);
    w.write_all(&frame)
}

/// Reads chunks up to and including the last.
pub fn read_message(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut message = Vec::new();
    loop {
        let mut head = [0u8; 5];
        r.read_exact(&mut head)?;
        let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        if len == 0 || message.len() + len > MAX_RESPONSE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad frame from the 1Password app"));
        }
        let start = message.len();
        message.resize(start + len - 1, 0);
        r.read_exact(&mut message[start..])?;
        match head[4] {
            LAST => return Ok(message),
            MORE => {}
            flag => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad chunk flag {flag} from the 1Password app")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_round_trip() {
        for len in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK + 7] {
            let message: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let mut wire = Vec::new();
            write_message(&mut wire, &message).unwrap();
            let frames = len.div_ceil(CHUNK).max(1);
            assert_eq!(wire.len(), len + 5 * frames);
            // As the app's own client writes them: full chunks flagged 2,
            // the last flagged 1.
            if len > CHUNK {
                assert_eq!(&wire[..5], &[0x20, 0xa1, 0x07, 0x00, MORE]);
            }
            assert_eq!(read_message(&mut wire.as_slice()).unwrap(), message);
        }
    }

    #[test]
    fn payloads_as_text_or_bytes() {
        for wire in [r#"{"success":true,"payload":"aGk="}"#, r#"{"success":true,"payload":[104,105]}"#] {
            assert_eq!(serde_json::from_str::<Response>(wire).unwrap().payload, b"hi");
        }
        assert!(serde_json::from_str::<Response>(r#"{"success":false,"payload":null}"#).unwrap().payload.is_empty());
        assert!(serde_json::from_str::<Response>(r#"{"success":false}"#).unwrap().payload.is_empty());
    }

    #[test]
    fn small_request_matches_the_official_client() {
        // Captured from libop_sdk_ipc_client: one frame, flag 1.
        let mut wire = Vec::new();
        write_message(&mut wire, b"{}").unwrap();
        assert_eq!(wire, [3, 0, 0, 0, 1, b'{', b'}']);
    }

    #[test]
    fn rejects_bad_frames() {
        assert!(read_message(&mut [0u8, 0, 0, 0, 1].as_slice()).is_err());
        assert!(read_message(&mut [2u8, 0, 0, 0, 9, b'x'].as_slice()).is_err());
        assert!(read_message(&mut [5u8, 0, 0, 0, 1, b'x'].as_slice()).is_err());
    }
}
