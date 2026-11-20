//! A client's whole TLS offer, as a host reads it.
//!
//! A host (and the network in front of it) sees a program's ClientHello
//! before any request, and can tell one TLS library from another by it:
//! which suites, which extensions, in what order, with what in them. The
//! recorder's acceptor (rustls's) gives only the suites, the signature
//! schemes, the ALPN and the name; this reads the message itself off the
//! connection before the acceptor has it, so a capture keeps the whole
//! offer (`capture::Hello::offer`) and the pool's own can be set beside
//! it (docs/reports/token-pool-tls.md).
//!
//! What's kept is what a fingerprint is made of, never what is one
//! connection's alone or could resume a session: no random, no session
//! id (its length), no key shares (their groups and lengths), no
//! session ticket or pre-shared key (their lengths), no name (the
//! capture has the host).

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};

/// The most of a connection read for its ClientHello: one with a
/// post-quantum key share is under 2 KB, a record at most 16 KB.
const MOST: usize = 1 << 16;

/// A ClientHello, in the client's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    /// The version in the message's own field (0x0303 since TLS 1.3:
    /// the versions offered are an extension's).
    pub version: u16,
    /// How long its session id is (32 in a browser's, for middleboxes;
    /// rustls's too).
    pub session_id: usize,
    /// Every cipher suite, GREASE values and signalling ones among them.
    pub suites: Vec<u16>,
    pub compressions: Vec<u8>,
    pub extensions: Vec<Extension>,
}

/// One extension of a ClientHello.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extension {
    pub kind: u16,
    /// Its data's length.
    pub len: usize,
    /// Its data in hex; for key shares, each share's group and its key's
    /// length ("001d:32,11ec:1216"); None where the data is one
    /// connection's alone, a secret's stand-in, or the host's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

const SERVER_NAME: u16 = 0;
const PADDING: u16 = 21;
const SESSION_TICKET: u16 = 35;
const PRE_SHARED_KEY: u16 = 41;
const EARLY_DATA: u16 = 42;
const COOKIE: u16 = 44;
const KEY_SHARE: u16 = 51;
const ENCRYPTED_CLIENT_HELLO: u16 = 0xfe0d;

/// Whether `v` is one of the values a client puts in to keep hosts
/// tolerant of what they don't know (RFC 8701): another at each
/// connection, so two offers are compared with them named, not valued.
pub fn grease(v: u16) -> bool {
    v & 0x0f0f == 0x0a0a && v >> 8 == v & 0xff
}

/// A reader over a message's bytes: each take is None past the end.
struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    /// A vector with a length of `width` bytes before it.
    fn vector(&mut self, width: usize) -> Option<&'a [u8]> {
        let n = match width {
            1 => usize::from(self.u8()?),
            _ => usize::from(self.u16()?),
        };
        self.take(n)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A key share extension's groups and key lengths, its keys left out.
fn shares(data: &[u8]) -> Option<String> {
    let mut b = Bytes(data);
    let mut list = Bytes(b.vector(2)?);
    let mut out = Vec::new();
    while !list.0.is_empty() {
        let group = list.u16()?;
        let key = list.vector(2)?;
        out.push(format!("{group:04x}:{}", key.len()));
    }
    Some(out.join(","))
}

impl Offer {
    /// The offer in a ClientHello handshake message (its four bytes of
    /// type and length first). None if it isn't one, or is cut short.
    pub fn parse(message: &[u8]) -> Option<Offer> {
        let mut b = Bytes(message);
        if b.u8()? != 1 {
            return None;
        }
        let len = b.take(3)?;
        let len = usize::from(len[0]) << 16 | usize::from(len[1]) << 8 | usize::from(len[2]);
        let mut b = Bytes(b.take(len)?);
        let version = b.u16()?;
        b.take(32)?;
        let session_id = b.vector(1)?.len();
        let suites = b.vector(2)?.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        let compressions = b.vector(1)?.to_vec();
        let mut extensions = Vec::new();
        // (A ClientHello from before extensions ends here.)
        if !b.0.is_empty() {
            let mut list = Bytes(b.vector(2)?);
            while !list.0.is_empty() {
                let kind = list.u16()?;
                let data = list.vector(2)?;
                let body = match kind {
                    SERVER_NAME | PADDING | SESSION_TICKET | PRE_SHARED_KEY | EARLY_DATA | COOKIE | ENCRYPTED_CLIENT_HELLO => None,
                    KEY_SHARE => shares(data),
                    _ => Some(hex(data)),
                };
                extensions.push(Extension { kind, len: data.len(), body });
            }
        }
        Some(Offer { version, session_id, suites, compressions, extensions })
    }

    /// The offer in the records a program wrote first (one or several,
    /// each with its five bytes of header): its own ClientHello, taken
    /// from what its TLS library would send (`relay::own_offer`).
    pub fn from_records(mut records: &[u8]) -> Option<Offer> {
        let mut message = Vec::new();
        while records.len() >= 5 && records[0] == 22 {
            let len = usize::from(u16::from_be_bytes([records[3], records[4]]));
            message.extend_from_slice(records.get(5..5 + len)?);
            records = &records[5 + len..];
        }
        Offer::parse(&message)
    }

    /// The offer on lines, GREASE values named and not valued: what two
    /// offers are compared by (`differences`), and what's printed.
    pub fn shape(&self) -> Vec<String> {
        let named = |v: u16| if grease(v) { "GREASE".to_owned() } else { format!("{v:04x}") };
        let mut out = vec![
            format!("version {:04x}", self.version),
            format!("session id of {}", self.session_id),
            format!("suites {}", self.suites.iter().map(|s| named(*s)).collect::<Vec<_>>().join(" ")),
            format!("compressions {}", hex(&self.compressions)),
            format!("extensions {}", self.extensions.iter().map(|e| named(e.kind)).collect::<Vec<_>>().join(" ")),
        ];
        for e in self.extensions.iter().filter(|e| !grease(e.kind)) {
            let body = match (e.kind, &e.body) {
                // (Lists of two-byte values, where GREASE may sit:
                // groups, versions, key shares.)
                (10, Some(b)) => named_list(b, 4),
                (43, Some(b)) => named_list(b, 2),
                (KEY_SHARE, Some(b)) => {
                    b.split(',').map(|s| if s.get(..4).and_then(|g| u16::from_str_radix(g, 16).ok()).is_some_and(grease) { "GREASE".to_owned() } else { s.to_owned() }).collect::<Vec<_>>().join(",")
                }
                (_, Some(b)) => b.clone(),
                (_, None) => format!("({} bytes)", e.len),
            };
            out.push(format!("{:04x} {body}", e.kind));
        }
        out
    }

    /// Where `other` isn't this offer, a line for each ("suites: … | …"):
    /// none when a host couldn't tell the two apart by their ClientHello.
    /// (Padding is left out: it fills to a length, and follows the rest.)
    pub fn differences(&self, other: &Offer) -> Vec<String> {
        let key = |l: &str| l.split(' ').next().unwrap_or_default().to_owned();
        let (a, b) = (self.shape(), other.shape());
        let mut out = Vec::new();
        for line in &a {
            let k = key(line);
            if k == format!("{PADDING:04x}") {
                continue;
            }
            match b.iter().find(|l| key(l) == k) {
                Some(theirs) if theirs == line => {}
                // (The same extensions in another order is said as
                // that: rustls writes them in a new order at each
                // connection, so two of its offers never agree on it,
                // and a host can't go by it.)
                Some(theirs) if k == "extensions" && sorted(line) == sorted(theirs) => out.push(format!("{ORDER}: {line} | {theirs}")),
                Some(theirs) => out.push(format!("{line} | {theirs}")),
                None => out.push(format!("{line} | (none)")),
            }
        }
        for line in &b {
            let k = key(line);
            if k != format!("{PADDING:04x}") && !a.iter().any(|l| key(l) == k) {
                out.push(format!("(none) | {line}"));
            }
        }
        out
    }
}

/// What `Offer::differences` says first of two offers with the same
/// extensions in another order.
pub const ORDER: &str = "the same extensions in another order";

/// A line's words in an order of their own, to compare what's there
/// whatever its order.
fn sorted(line: &str) -> Vec<&str> {
    let mut words: Vec<&str> = line.split(' ').collect();
    words.sort_unstable();
    words
}

/// A hex list of two-byte values after `skip` hex digits of length,
/// GREASE values named.
fn named_list(body: &str, skip: usize) -> String {
    let values = body.get(skip..).unwrap_or_default();
    (0..values.len() / 4).map(|i| &values[i * 4..i * 4 + 4]).map(|v| if u16::from_str_radix(v, 16).is_ok_and(grease) { "GREASE".to_owned() } else { v.to_owned() }).collect::<Vec<_>>().join(" ")
}

/// Reads a connection's ClientHello off `stream`: the bytes read (the
/// records whole, to be given to the acceptor before the rest of the
/// stream) and the offer in them, if they were one. Reads no further
/// than the message's last record. None if the stream ended first.
pub async fn read<S: AsyncRead + Unpin>(stream: &mut S) -> Option<(Vec<u8>, Option<Offer>)> {
    let mut raw = Vec::new();
    let mut message = Vec::new();
    loop {
        let mut head = [0u8; 5];
        stream.read_exact(&mut head).await.ok()?;
        raw.extend_from_slice(&head);
        let len = usize::from(u16::from_be_bytes([head[3], head[4]]));
        // (Not a handshake record, or more than any ClientHello: the
        // acceptor's to refuse, with what was read.)
        if head[0] != 22 || raw.len() + len > MOST {
            return Some((raw, None));
        }
        let at = raw.len();
        raw.resize(at + len, 0);
        stream.read_exact(&mut raw[at..]).await.ok()?;
        message.extend_from_slice(&raw[at..]);
        // (A message may run over several records: its own length says
        // when it's whole.)
        if message.len() >= 4 {
            let whole = 4 + (usize::from(message[1]) << 16 | usize::from(message[2]) << 8 | usize::from(message[3]));
            if message[0] != 1 {
                return Some((raw, None));
            }
            if message.len() >= whole {
                return Some((raw, Offer::parse(&message[..whole])));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ClientHello as rustls writes one (the pool's own library), in
    /// its record.
    fn rustls_hello() -> Vec<u8> {
        let crypto = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut config =
            rustls::ClientConfig::builder_with_provider(crypto).with_safe_default_protocol_versions().unwrap().with_root_certificates(rustls::RootCertStore::empty()).with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let mut conn = rustls::ClientConnection::new(std::sync::Arc::new(config), "example.com".try_into().unwrap()).unwrap();
        let mut out = Vec::new();
        conn.write_tls(&mut out).unwrap();
        out
    }

    #[tokio::test]
    async fn a_hello_is_read_whole_and_no_further() {
        let mut bytes = rustls_hello();
        let hello = bytes.len();
        bytes.extend_from_slice(b"what comes after");
        let mut stream = &bytes[..];
        let (raw, offer) = read(&mut stream).await.unwrap();
        assert_eq!(raw, bytes[..hello]);
        assert_eq!(stream, b"what comes after");
        let offer = offer.unwrap();
        assert_eq!(offer.version, 0x0303);
        // TLS 1.3's three first, as the provider orders them.
        assert_eq!(offer.suites[..3], [0x1302, 0x1301, 0x1303]);
        let kinds: Vec<u16> = offer.extensions.iter().map(|e| e.kind).collect();
        for kind in [SERVER_NAME, 10, 13, 16, 43, KEY_SHARE] {
            assert!(kinds.contains(&kind), "{kind} among {kinds:?}");
        }
        // ALPN as it was offered.
        let alpn = offer.extensions.iter().find(|e| e.kind == 16).unwrap();
        assert_eq!(alpn.body.as_deref(), Some("000c02683208687474702f312e31"));
    }

    /// Nothing of one connection's alone is kept: the name, the keys.
    #[tokio::test]
    async fn what_is_one_connections_is_left_out() {
        let bytes = rustls_hello();
        let (_, offer) = read(&mut &bytes[..]).await.unwrap();
        let offer = offer.unwrap();
        assert_eq!(offer.extensions.iter().find(|e| e.kind == SERVER_NAME).unwrap().body, None);
        let share = offer.extensions.iter().find(|e| e.kind == KEY_SHARE).unwrap();
        let body = share.body.as_deref().unwrap();
        assert!(body.split(',').all(|s| s.len() < 12 && s.contains(':')), "{body}");
        assert!(body.len() < share.len);
        // (And two connections of one program offer the same, but for
        // the order of its extensions: rustls writes them in a new
        // order each time.)
        let orders: std::collections::HashSet<Vec<u16>> = (0..8).map(|_| Offer::parse(&rustls_hello()[5..]).unwrap().extensions.iter().map(|e| e.kind).collect()).collect();
        assert!(orders.len() > 1, "rustls no longer shuffles its extensions: say so in docs/reports/token-pool-tls.md");
        let (_, again) = read(&mut &rustls_hello()[..]).await.unwrap();
        assert!(offer.differences(&again.unwrap()).iter().all(|d| d.starts_with(ORDER)));
    }

    /// A message over two records is put together; what isn't a
    /// ClientHello is handed back as read.
    #[tokio::test]
    async fn over_two_records_and_not_a_hello() {
        let bytes = rustls_hello();
        let (head, body) = bytes.split_at(5);
        let (first, second) = body.split_at(40);
        let record = |part: &[u8]| [&[22, head[1], head[2]][..], &(part.len() as u16).to_be_bytes(), part].concat();
        let split = [record(first), record(second)].concat();
        let (raw, offer) = read(&mut &split[..]).await.unwrap();
        assert_eq!(raw, split);
        assert_eq!(offer, read(&mut &bytes[..]).await.unwrap().1);
        let (raw, offer) = read(&mut &b"GET / HTTP/1.1\r\n\r\n"[..]).await.unwrap();
        assert_eq!((raw.as_slice(), offer), (&b"GET /"[..], None));
        assert!(read(&mut &bytes[..20]).await.is_none());
    }

    #[test]
    fn two_offers_apart() {
        let a = Offer {
            version: 0x0303,
            session_id: 32,
            suites: vec![0x1a1a, 0x1301],
            compressions: vec![0],
            extensions: vec![Extension { kind: 10, len: 8, body: Some("00069a9a001d0017".into()) }, Extension { kind: PADDING, len: 100, body: None }],
        };
        // GREASE of another value is the same offer; padding doesn't count.
        let mut b = a.clone();
        b.suites[0] = 0x4a4a;
        b.extensions[0].body = Some("0006caca001d0017".into());
        b.extensions[1].len = 7;
        assert!(grease(0x1a1a) && grease(0xfafa) && !grease(0x1301) && !grease(0x1a2a));
        assert_eq!(a.differences(&b), Vec::<String>::new());
        // Another order of groups, and an extension the other hasn't.
        b.extensions[0].body = Some("00069a9a0017001d".into());
        b.extensions.push(Extension { kind: 0x4469, len: 5, body: Some("0003026832".into()) });
        assert_eq!(a.differences(&b), ["extensions 000a 0015 | extensions 000a 0015 4469", "000a GREASE 001d 0017 | 000a GREASE 0017 001d", "(none) | 4469 0003026832"]);
        // The same extensions the other way round.
        let mut c = a.clone();
        c.extensions.reverse();
        assert_eq!(a.differences(&c), [format!("{ORDER}: extensions 000a 0015 | extensions 0015 000a")]);
    }
}
