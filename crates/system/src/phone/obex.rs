//! OBEX (IrDA's Object Exchange, as Bluetooth's GOEP uses it over
//! RFCOMM): the packets, a client's session ([`Client`]: Connect, Get,
//! Put, SetPath, Disconnect) and a server's ([`Server`]: one whole
//! operation at a time), over anything that reads and writes (a Bluetooth
//! socket; a socket pair in tests). Written from the specification (IrOBEX
//! 1.5, GOEP 2.1); single-response mode is not used (it is L2CAP's).
//!
//! A packet is an opcode, its whole length in two bytes, and headers; a
//! header is an id whose top two bits say how its value is held (text in
//! UTF-16 with a length, bytes with a length, one byte, four bytes).

use std::io::{self, Read, Write};

/// The largest packet Olo takes (and says it takes).
pub const MAX_PACKET: usize = 0xFFFF;
/// The least any OBEX peer must take.
const MIN_PACKET: usize = 255;
/// The most an object fetched or received may hold (a phone book with
/// its pictures left out, a listing, a message: far less).
pub const MAX_OBJECT: usize = 32 << 20;

pub mod op {
    pub const CONNECT: u8 = 0x80;
    pub const DISCONNECT: u8 = 0x81;
    pub const PUT: u8 = 0x02;
    pub const GET: u8 = 0x03;
    pub const SETPATH: u8 = 0x85;
    pub const ABORT: u8 = 0xFF;
    /// The bit that says a request is whole.
    pub const FINAL: u8 = 0x80;
}

pub mod code {
    pub const CONTINUE: u8 = 0x90;
    pub const SUCCESS: u8 = 0xA0;
    pub const BAD_REQUEST: u8 = 0xC0;
    pub const UNAUTHORIZED: u8 = 0xC1;
    pub const FORBIDDEN: u8 = 0xC3;
    pub const NOT_FOUND: u8 = 0xC4;
    pub const NOT_ACCEPTABLE: u8 = 0xC6;
    pub const PRECONDITION_FAILED: u8 = 0xCC;
    pub const NOT_IMPLEMENTED: u8 = 0xD1;
    pub const SERVICE_UNAVAILABLE: u8 = 0xD3;
}

pub mod id {
    pub const NAME: u8 = 0x01;
    pub const TYPE: u8 = 0x42;
    pub const TARGET: u8 = 0x46;
    pub const BODY: u8 = 0x48;
    pub const END_OF_BODY: u8 = 0x49;
    pub const WHO: u8 = 0x4A;
    pub const APP_PARAMS: u8 = 0x4C;
    pub const LENGTH: u8 = 0xC3;
    pub const CONNECTION_ID: u8 = 0xCB;
}

/// What went wrong with an exchange.
#[derive(Debug)]
pub enum Error {
    /// The link (closed, timed out).
    Io(io::Error),
    /// The other side said no: its response code (`code::FORBIDDEN`: the
    /// phone hasn't allowed this yet).
    Refused(u8),
    /// What came wasn't OBEX, or was more than is taken.
    Malformed(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "the link: {e}"),
            Error::Refused(code::FORBIDDEN | code::UNAUTHORIZED) => write!(f, "the phone hasn't allowed it"),
            Error::Refused(code::NOT_FOUND) => write!(f, "the phone has no such thing"),
            Error::Refused(code::SERVICE_UNAVAILABLE) => write!(f, "the phone can't just now"),
            Error::Refused(c) => write!(f, "the phone refused (OBEX {c:#04x})"),
            Error::Malformed(what) => write!(f, "not understood: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A header's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Bytes(Vec<u8>),
    Byte(u8),
    Four(u32),
}

/// A packet's headers, in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Headers(pub Vec<(u8, Value)>);

impl Headers {
    pub fn new() -> Headers {
        Headers::default()
    }

    pub fn text(mut self, id: u8, text: &str) -> Headers {
        self.0.push((id, Value::Text(text.to_owned())));
        self
    }

    pub fn bytes(mut self, id: u8, bytes: impl Into<Vec<u8>>) -> Headers {
        self.0.push((id, Value::Bytes(bytes.into())));
        self
    }

    pub fn four(mut self, id: u8, value: u32) -> Headers {
        self.0.push((id, Value::Four(value)));
        self
    }

    /// With the connection's id first, as it must be, if there is one.
    fn on(mut self, connection: Option<u32>) -> Headers {
        if let Some(c) = connection {
            self.0.insert(0, (id::CONNECTION_ID, Value::Four(c)));
        }
        self
    }

    pub fn get(&self, id: u8) -> Option<&Value> {
        self.0.iter().find(|(i, _)| *i == id).map(|(_, v)| v)
    }

    pub fn get_bytes(&self, id: u8) -> Option<&[u8]> {
        match self.get(id)? {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn get_text(&self, id: u8) -> Option<&str> {
        match self.get(id)? {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    /// A Type header's value: ASCII, with its closing NUL taken off.
    pub fn kind(&self) -> Option<String> {
        self.get_bytes(id::TYPE).map(|b| String::from_utf8_lossy(b).trim_end_matches('\0').to_ascii_lowercase())
    }

    fn encode(&self, out: &mut Vec<u8>) {
        for (id, value) in &self.0 {
            out.push(*id);
            match value {
                Value::Text(t) => {
                    // UTF-16, big-endian, closed with a NUL; an empty
                    // name is the header alone.
                    let units: Vec<u16> = if t.is_empty() { Vec::new() } else { t.encode_utf16().chain(std::iter::once(0)).collect() };
                    out.extend(((3 + 2 * units.len()) as u16).to_be_bytes());
                    out.extend(units.iter().flat_map(|u| u.to_be_bytes()));
                }
                Value::Bytes(b) => {
                    out.extend(((3 + b.len()) as u16).to_be_bytes());
                    out.extend(b);
                }
                Value::Byte(b) => out.push(*b),
                Value::Four(v) => out.extend(v.to_be_bytes()),
            }
        }
    }

    fn len(&self) -> usize {
        let mut v = Vec::new();
        self.encode(&mut v);
        v.len()
    }

    fn decode(mut b: &[u8]) -> Result<Headers> {
        let mut out = Vec::new();
        while let Some((&id, rest)) = b.split_first() {
            let (value, used) = match id >> 6 {
                0 | 1 => {
                    let len = rest.get(..2).map(|l| u16::from_be_bytes([l[0], l[1]]) as usize).ok_or(Error::Malformed("a header's length"))?;
                    let data = rest.get(2..len.checked_sub(1).ok_or(Error::Malformed("a header's length"))?).ok_or(Error::Malformed("a header cut short"))?;
                    let value = if id >> 6 == 0 {
                        let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).take_while(|u| *u != 0).collect();
                        Value::Text(String::from_utf16_lossy(&units))
                    } else {
                        Value::Bytes(data.to_vec())
                    };
                    (value, len - 1)
                }
                2 => (Value::Byte(*rest.first().ok_or(Error::Malformed("a header cut short"))?), 1),
                _ => (Value::Four(u32::from_be_bytes(rest.get(..4).ok_or(Error::Malformed("a header cut short"))?.try_into().expect("four"))), 4),
            };
            out.push((id, value));
            b = &rest[used..];
        }
        Ok(Headers(out))
    }
}

/// Application parameters (a header of tag, length, value triples), as
/// the profiles over OBEX use them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Params(pub Vec<(u8, Vec<u8>)>);

impl Params {
    pub fn new() -> Params {
        Params::default()
    }

    pub fn byte(mut self, tag: u8, v: u8) -> Params {
        self.0.push((tag, vec![v]));
        self
    }

    pub fn two(mut self, tag: u8, v: u16) -> Params {
        self.0.push((tag, v.to_be_bytes().to_vec()));
        self
    }

    pub fn eight(mut self, tag: u8, v: u64) -> Params {
        self.0.push((tag, v.to_be_bytes().to_vec()));
        self
    }

    pub fn encode(&self) -> Vec<u8> {
        self.0.iter().flat_map(|(t, v)| [*t, v.len() as u8].into_iter().chain(v.iter().copied())).collect()
    }

    pub fn decode(mut b: &[u8]) -> Params {
        let mut out = Vec::new();
        while b.len() >= 2 {
            let (tag, len) = (b[0], b[1] as usize);
            let Some(v) = b.get(2..2 + len) else { break };
            out.push((tag, v.to_vec()));
            b = &b[2 + len..];
        }
        Params(out)
    }

    pub fn get(&self, tag: u8) -> Option<&[u8]> {
        self.0.iter().find(|(t, _)| *t == tag).map(|(_, v)| v.as_slice())
    }

    pub fn get_byte(&self, tag: u8) -> Option<u8> {
        self.get(tag).and_then(|v| v.first().copied())
    }

    pub fn get_two(&self, tag: u8) -> Option<u16> {
        self.get(tag).filter(|v| v.len() == 2).map(|v| u16::from_be_bytes([v[0], v[1]]))
    }
}

/// One packet read: its first byte (an opcode or a response code) and
/// the rest after its length.
fn read_packet(link: &mut impl Read) -> Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 3];
    link.read_exact(&mut head)?;
    let len = u16::from_be_bytes([head[1], head[2]]) as usize;
    if len < 3 {
        return Err(Error::Malformed("a packet shorter than its own head"));
    }
    let mut rest = vec![0u8; len - 3];
    link.read_exact(&mut rest)?;
    Ok((head[0], rest))
}

fn write_packet(link: &mut impl Write, first: u8, fixed: &[u8], headers: &Headers) -> Result<()> {
    let mut body = fixed.to_vec();
    headers.encode(&mut body);
    let len = 3 + body.len();
    if len > MAX_PACKET {
        return Err(Error::Malformed("a packet too large to send"));
    }
    let mut packet = Vec::with_capacity(len);
    packet.push(first);
    packet.extend((len as u16).to_be_bytes());
    packet.extend(body);
    link.write_all(&packet)?;
    link.flush()?;
    Ok(())
}

/// A session with an OBEX server (the phone's messages, its contacts).
pub struct Client<L: Read + Write> {
    link: L,
    connection: Option<u32>,
    /// The largest packet the server takes.
    max: usize,
}

impl<L: Read + Write> Client<L> {
    /// Connect to the service `target` names (a profile's 16 bytes).
    pub fn connect(mut link: L, target: &[u8; 16]) -> Result<Client<L>> {
        let fixed = [0x10, 0x00, (MAX_PACKET >> 8) as u8, MAX_PACKET as u8];
        write_packet(&mut link, op::CONNECT, &fixed, &Headers::new().bytes(id::TARGET, target.to_vec()))?;
        let (code, rest) = read_packet(&mut link)?;
        if code != code::SUCCESS {
            return Err(Error::Refused(code));
        }
        let fixed = rest.get(..4).ok_or(Error::Malformed("a Connect answer cut short"))?;
        let max = (u16::from_be_bytes([fixed[2], fixed[3]]) as usize).clamp(MIN_PACKET, MAX_PACKET);
        let headers = Headers::decode(&rest[4..])?;
        let connection = match headers.get(id::CONNECTION_ID) {
            Some(Value::Four(c)) => Some(*c),
            _ => None,
        };
        Ok(Client { link, connection, max })
    }

    fn response(&mut self) -> Result<(u8, Headers)> {
        let (code, rest) = read_packet(&mut self.link)?;
        Ok((code, Headers::decode(&rest)?))
    }

    /// Fetch an object: its body, and the answer's headers (application
    /// parameters among them).
    pub fn get(&mut self, kind: &str, name: Option<&str>, params: &Params) -> Result<(Vec<u8>, Headers)> {
        let mut headers = Headers::new().on(self.connection);
        if let Some(name) = name {
            headers = headers.text(id::NAME, name);
        }
        headers = headers.bytes(id::TYPE, [kind.as_bytes(), &[0]].concat());
        if !params.0.is_empty() {
            headers = headers.bytes(id::APP_PARAMS, params.encode());
        }
        write_packet(&mut self.link, op::GET | op::FINAL, &[], &headers)?;
        let (mut body, mut first) = (Vec::new(), None);
        loop {
            let (code, headers) = self.response()?;
            for (id, value) in &headers.0 {
                if let (id::BODY | id::END_OF_BODY, Value::Bytes(b)) = (*id, value) {
                    if body.len() + b.len() > MAX_OBJECT {
                        let _ = write_packet(&mut self.link, op::ABORT, &[], &Headers::new().on(self.connection));
                        return Err(Error::Malformed("an object larger than is taken"));
                    }
                    body.extend(b);
                }
            }
            let first = first.get_or_insert(headers);
            match code {
                code::CONTINUE => write_packet(&mut self.link, op::GET | op::FINAL, &[], &Headers::new().on(self.connection))?,
                code::SUCCESS => return Ok((body, first.clone())),
                c => return Err(Error::Refused(c)),
            }
        }
    }

    /// Send an object; the answer's headers (a Name: what the server
    /// calls what it took).
    pub fn put(&mut self, kind: &str, name: Option<&str>, params: &Params, body: &[u8]) -> Result<Headers> {
        let mut headers = Headers::new().on(self.connection);
        if let Some(name) = name {
            headers = headers.text(id::NAME, name);
        }
        headers = headers.bytes(id::TYPE, [kind.as_bytes(), &[0]].concat());
        if !params.0.is_empty() {
            headers = headers.bytes(id::APP_PARAMS, params.encode());
        }
        // The body in as many packets as it takes: each but the last a
        // Put answered Continue, the last a final Put with what's left.
        let mut rest = body;
        loop {
            // (3 for the packet's head, 3 for the body header's.)
            let room = self.max.saturating_sub(3 + headers.len() + 3);
            if room == 0 {
                return Err(Error::Malformed("headers larger than a packet"));
            }
            if rest.len() <= room {
                write_packet(&mut self.link, op::PUT | op::FINAL, &[], &headers.bytes(id::END_OF_BODY, rest.to_vec()))?;
                let (code, answer) = self.response()?;
                return if code == code::SUCCESS { Ok(answer) } else { Err(Error::Refused(code)) };
            }
            let (now, later) = rest.split_at(room);
            write_packet(&mut self.link, op::PUT, &[], &headers.bytes(id::BODY, now.to_vec()))?;
            let (code, _) = self.response()?;
            if code != code::CONTINUE {
                return Err(Error::Refused(code));
            }
            rest = later;
            headers = Headers::new().on(self.connection);
        }
    }

    /// Go into folder `name` of the current one; `None`: back to the top.
    pub fn set_path(&mut self, name: Option<&str>) -> Result<()> {
        // (Flags: don't make a folder that isn't there.)
        let headers = Headers::new().on(self.connection).text(id::NAME, name.unwrap_or_default());
        write_packet(&mut self.link, op::SETPATH, &[0x02, 0x00], &headers)?;
        match self.response()? {
            (code::SUCCESS, _) => Ok(()),
            (c, _) => Err(Error::Refused(c)),
        }
    }

    /// End the session (the link stays the caller's to close).
    pub fn disconnect(mut self) -> L {
        if write_packet(&mut self.link, op::DISCONNECT, &[], &Headers::new().on(self.connection)).is_ok() {
            let _ = read_packet(&mut self.link);
        }
        self.link
    }

    pub fn link(&self) -> &L {
        &self.link
    }
}

/// What a client asked of a [`Server`], whole.
#[derive(Debug, PartialEq, Eq)]
pub enum Asked {
    /// A session, for the service `target` names (answer with
    /// [`Server::accept`] or [`Server::refuse`]).
    Connect { target: Option<Vec<u8>> },
    /// An object sent, all of it (already answered Continue as it came:
    /// answer the last of it with [`Server::done`] or `refuse`).
    Put { headers: Headers, body: Vec<u8> },
    /// An object wanted (answer with [`Server::send`] or `refuse`).
    Get { headers: Headers },
    /// A folder to go into (`None`: the top; answer with `done`).
    SetPath { name: Option<String>, up: bool },
    /// The session ended (answered).
    Disconnect,
}

/// The server's end of a session: what a phone connects to to tell of
/// messages as they come (and the stand-in phone, in tests).
pub struct Server<L: Read + Write> {
    link: L,
    /// The largest packet the client takes.
    max: usize,
}

impl<L: Read + Write> Server<L> {
    pub fn new(link: L) -> Server<L> {
        Server { link, max: MIN_PACKET }
    }

    /// The next thing asked, whole. An error: the link has gone.
    pub fn next(&mut self) -> Result<Asked> {
        let (mut put_headers, mut put_body): (Option<Headers>, Vec<u8>) = (None, Vec::new());
        loop {
            let (opcode, rest) = read_packet(&mut self.link)?;
            match opcode {
                op::CONNECT => {
                    let fixed = rest.get(..4).ok_or(Error::Malformed("a Connect cut short"))?;
                    self.max = (u16::from_be_bytes([fixed[2], fixed[3]]) as usize).clamp(MIN_PACKET, MAX_PACKET);
                    let headers = Headers::decode(&rest[4..])?;
                    return Ok(Asked::Connect { target: headers.get_bytes(id::TARGET).map(<[u8]>::to_vec) });
                }
                op::DISCONNECT => {
                    write_packet(&mut self.link, code::SUCCESS, &[], &Headers::new())?;
                    return Ok(Asked::Disconnect);
                }
                op::SETPATH => {
                    let flags = *rest.first().ok_or(Error::Malformed("a SetPath cut short"))?;
                    let headers = Headers::decode(rest.get(2..).unwrap_or_default())?;
                    let name = headers.get_text(id::NAME).filter(|n| !n.is_empty()).map(str::to_owned);
                    return Ok(Asked::SetPath { name, up: flags & 1 != 0 });
                }
                o if o & !op::FINAL == op::PUT => {
                    let headers = Headers::decode(&rest)?;
                    for (id, value) in &headers.0 {
                        if let (id::BODY | id::END_OF_BODY, Value::Bytes(b)) = (*id, value) {
                            if put_body.len() + b.len() > MAX_OBJECT {
                                self.refuse(code::NOT_ACCEPTABLE)?;
                                return Err(Error::Malformed("an object larger than is taken"));
                            }
                            put_body.extend(b);
                        }
                    }
                    let first = put_headers.get_or_insert(headers);
                    if o & op::FINAL != 0 {
                        return Ok(Asked::Put { headers: std::mem::take(first), body: put_body });
                    }
                    write_packet(&mut self.link, code::CONTINUE, &[], &Headers::new())?;
                }
                o if o & !op::FINAL == op::GET => {
                    let headers = Headers::decode(&rest)?;
                    if o & op::FINAL != 0 {
                        return Ok(Asked::Get { headers });
                    }
                    write_packet(&mut self.link, code::CONTINUE, &[], &Headers::new())?;
                }
                op::ABORT => write_packet(&mut self.link, code::SUCCESS, &[], &Headers::new())?,
                _ => write_packet(&mut self.link, code::NOT_IMPLEMENTED, &[], &Headers::new())?,
            }
        }
    }

    /// Yes to a Connect: as the service `who` names, the session `id`.
    pub fn accept(&mut self, who: Option<&[u8]>, id: u32) -> Result<()> {
        let mut headers = Headers::new().four(id::CONNECTION_ID, id);
        if let Some(who) = who {
            headers = headers.bytes(id::WHO, who.to_vec());
        }
        let fixed = [0x10, 0x00, (MAX_PACKET >> 8) as u8, MAX_PACKET as u8];
        write_packet(&mut self.link, code::SUCCESS, &fixed, &headers)
    }

    /// Done (a Put taken, a folder gone into), with what to say of it.
    pub fn done(&mut self, headers: &Headers) -> Result<()> {
        write_packet(&mut self.link, code::SUCCESS, &[], headers)
    }

    pub fn refuse(&mut self, code: u8) -> Result<()> {
        write_packet(&mut self.link, code, &[], &Headers::new())
    }

    /// The answer to a Get: `body`, in as many packets as it takes, with
    /// `params` in the first.
    pub fn send(&mut self, params: &Params, body: &[u8]) -> Result<()> {
        let mut headers = Headers::new();
        if !params.0.is_empty() {
            headers = headers.bytes(id::APP_PARAMS, params.encode());
        }
        let mut rest = body;
        loop {
            let room = self.max.saturating_sub(3 + headers.len() + 3).max(1);
            if rest.len() <= room {
                return write_packet(&mut self.link, code::SUCCESS, &[], &headers.bytes(id::END_OF_BODY, rest.to_vec()));
            }
            let (now, later) = rest.split_at(room);
            write_packet(&mut self.link, code::CONTINUE, &[], &headers.bytes(id::BODY, now.to_vec()))?;
            // (The client asks for the next of it.)
            let (opcode, _) = read_packet(&mut self.link)?;
            if opcode & !op::FINAL != op::GET {
                // Given up on (an Abort), or something else: answered,
                // and the object let go.
                return write_packet(&mut self.link, code::SUCCESS, &[], &Headers::new());
            }
            rest = later;
            headers = Headers::new();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn headers_as_the_specification_writes_them() {
        // Connect with a Target, as GOEP's example: 80 00 1A 10 00 FF FF
        // 46 00 13 <16 bytes>.
        let target = [0xbb, 0x58, 0x2b, 0x40, 0x42, 0x0c, 0x11, 0xdb, 0xb0, 0xde, 0x08, 0x00, 0x20, 0x0c, 0x9a, 0x66];
        let mut out = Vec::new();
        write_packet(&mut out, op::CONNECT, &[0x10, 0x00, 0xFF, 0xFF], &Headers::new().bytes(id::TARGET, target.to_vec())).unwrap();
        assert_eq!(out[..10], [0x80, 0x00, 0x1A, 0x10, 0x00, 0xFF, 0xFF, 0x46, 0x00, 0x13]);
        assert_eq!(out.len(), 0x1A);
        // A Name is UTF-16 with a closing NUL; an empty one is its head.
        let mut h = Vec::new();
        Headers::new().text(id::NAME, "inbox").text(id::NAME, "").four(id::CONNECTION_ID, 7).encode(&mut h);
        assert_eq!(h[..3], [0x01, 0x00, 0x0F]);
        assert_eq!(&h[3..15], [0, b'i', 0, b'n', 0, b'b', 0, b'o', 0, b'x', 0, 0]);
        assert_eq!(h[15..], [0x01, 0x00, 0x03, 0xCB, 0, 0, 0, 7]);
        let back = Headers::decode(&h).unwrap();
        assert_eq!(back.0, [(id::NAME, Value::Text("inbox".into())), (id::NAME, Value::Text(String::new())), (id::CONNECTION_ID, Value::Four(7))]);
        // What's cut short is refused, never read past.
        assert!(Headers::decode(&[0x01, 0x00, 0x20, 0, b'i']).is_err());
        assert!(Headers::decode(&[0x48, 0x00]).is_err());
        assert!(Headers::decode(&[0xCB, 0, 0]).is_err());
        let p = Params::new().two(0x01, 20).byte(0x13, 255);
        assert_eq!(p.encode(), [0x01, 2, 0, 20, 0x13, 1, 255]);
        assert_eq!((Params::decode(&p.encode()).get_two(0x01), Params::decode(&[0x01, 9, 1]).0.len()), (Some(20), 0));
    }

    /// A client and a server, end to end over a socket pair: a session,
    /// folders, a Get and a Put each longer than a packet, a refusal.
    #[test]
    fn a_session_end_to_end() {
        let (a, b) = UnixStream::pair().unwrap();
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let served = big.clone();
        let server = std::thread::spawn(move || {
            let mut s = Server::new(b);
            let mut seen = Vec::new();
            loop {
                match s.next().unwrap() {
                    Asked::Connect { target } => {
                        assert_eq!(target.as_deref(), Some(&[7u8; 16][..]));
                        s.accept(Some(&[7u8; 16]), 42).unwrap();
                    }
                    Asked::SetPath { name, up } => {
                        seen.push(format!("path {name:?} {up}"));
                        s.done(&Headers::new()).unwrap();
                    }
                    Asked::Get { headers } => {
                        assert_eq!(headers.get(id::CONNECTION_ID), Some(&Value::Four(42)));
                        match headers.get_text(id::NAME) {
                            Some("big") => s.send(&Params::new().two(0x12, 3), &served).unwrap(),
                            Some("small") => s.send(&Params::new(), b"hello").unwrap(),
                            _ => s.refuse(code::NOT_FOUND).unwrap(),
                        }
                        seen.push(format!("get {:?} {:?}", headers.kind(), headers.get_text(id::NAME)));
                    }
                    Asked::Put { headers, body } => {
                        seen.push(format!("put {:?} {} bytes", headers.kind(), body.len()));
                        assert!(body == served || body == b"x");
                        s.done(&Headers::new().text(id::NAME, "handle-1")).unwrap();
                    }
                    Asked::Disconnect => return seen,
                }
            }
        });
        let mut c = Client::connect(a, &[7u8; 16]).unwrap();
        assert_eq!((c.connection, c.max), (Some(42), MAX_PACKET));
        c.set_path(None).unwrap();
        c.set_path(Some("telecom")).unwrap();
        let (body, headers) = c.get("x-bt/listing", Some("big"), &Params::new().two(0x01, 9)).unwrap();
        assert!(body == big);
        assert_eq!(Params::decode(headers.get_bytes(id::APP_PARAMS).unwrap()).get_two(0x12), Some(3));
        assert_eq!(c.get("x-bt/listing", Some("small"), &Params::new()).unwrap().0, b"hello");
        assert!(matches!(c.get("x-bt/listing", Some("nothing"), &Params::new()), Err(Error::Refused(code::NOT_FOUND))));
        let answer = c.put("x-bt/message", Some("outbox"), &Params::new().byte(0x14, 1), &big).unwrap();
        assert_eq!(answer.get_text(id::NAME), Some("handle-1"));
        c.put("x-bt/small", None, &Params::new(), b"x").unwrap();
        c.disconnect();
        assert_eq!(
            server.join().unwrap(),
            [
                "path None false",
                "path Some(\"telecom\") false",
                "get Some(\"x-bt/listing\") Some(\"big\")",
                "get Some(\"x-bt/listing\") Some(\"small\")",
                "get Some(\"x-bt/listing\") Some(\"nothing\")",
                "put Some(\"x-bt/message\") 200000 bytes",
                "put Some(\"x-bt/small\") 1 bytes",
            ]
        );
    }

    /// A server that says it takes only the least: a Put is cut to fit.
    #[test]
    fn packets_no_larger_than_the_other_side_takes() {
        let (a, b) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut link = b;
            let (opcode, _) = read_packet(&mut link).unwrap();
            assert_eq!(opcode, op::CONNECT);
            write_packet(&mut link, code::SUCCESS, &[0x10, 0, 0, 255], &Headers::new()).unwrap();
            let mut sizes = Vec::new();
            loop {
                let (opcode, rest) = read_packet(&mut link).unwrap();
                sizes.push(rest.len() + 3);
                let last = opcode & op::FINAL != 0;
                write_packet(&mut link, if last { code::SUCCESS } else { code::CONTINUE }, &[], &Headers::new()).unwrap();
                if last {
                    return sizes;
                }
            }
        });
        let mut c = Client::connect(a, &[1u8; 16]).unwrap();
        assert_eq!((c.connection, c.max), (None, 255));
        c.put("x-bt/message", Some("outbox"), &Params::new(), &[9u8; 600]).unwrap();
        let sizes = server.join().unwrap();
        assert!(sizes.len() >= 3 && sizes.iter().all(|s| *s <= 255), "{sizes:?}");
    }
}
