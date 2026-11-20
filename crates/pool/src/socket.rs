//! A WebSocket's frames read as they pass (RFC 6455), each message whole:
//! what the server says on a Codex socket the pool passes on (each turn's
//! start and end, its usage, a 429), its bytes passed on as they came. A
//! message's frames are held only until it's whole (a moment: Codex's
//! events are small, one frame each), and one compressed by
//! `permessage-deflate` (RFC 7692) is inflated for the pool's own reading,
//! the server's context kept from message to message as the client keeps
//! it. A socket it can't read (a frame it doesn't know, a message past
//! `MESSAGE_MOST`) goes on as it comes, read no further.

use flate2::{Decompress, FlushDecompress, Status};

/// The most of one message read (a turn's last event holds its whole
/// response); past it, the socket goes on unread.
const MESSAGE_MOST: usize = 32 << 20;

/// The server's side of a socket, read as it passes.
pub struct Reader {
    /// What came and hasn't been passed on: the current message's frames,
    /// held until it's whole, and what came after them.
    held: Vec<u8>,
    /// Where the next frame starts in `held`.
    at: usize,
    /// The message being read: text or not, compressed or not, its
    /// payload so far.
    message: Option<(bool, bool, Vec<u8>)>,
    /// The server's compression context (`permessage-deflate`), and
    /// whether it starts afresh for each message
    /// (`server_no_context_takeover`).
    inflate: Option<Decompress>,
    afresh: bool,
    /// Read no further: everything passed on as it comes.
    blind: bool,
}

/// What `Reader::push` gives: the bytes to pass on now (whole frames, as
/// they came), and the text messages among them.
#[derive(Debug, Default, PartialEq)]
pub struct Passed {
    pub bytes: Vec<u8>,
    pub messages: Vec<String>,
}

impl Reader {
    /// For a socket whose handshake agreed `extensions` (its 101's
    /// `Sec-WebSocket-Extensions`).
    pub fn new(extensions: Option<&str>) -> Reader {
        let deflate = extensions.and_then(|e| e.split(',').map(str::trim).find(|x| x.split(';').next().is_some_and(|n| n.trim().eq_ignore_ascii_case("permessage-deflate"))));
        Reader {
            held: Vec::new(),
            at: 0,
            message: None,
            inflate: deflate.map(|_| Decompress::new(false)),
            afresh: deflate.is_some_and(|d| d.split(';').any(|p| p.trim().eq_ignore_ascii_case("server_no_context_takeover"))),
            blind: false,
        }
    }

    /// Whether it has stopped reading (what it passes on is then unread).
    pub fn blind(&self) -> bool {
        self.blind
    }

    /// `bytes` came from the server: what to pass on now, and the
    /// messages that completes.
    pub fn push(&mut self, bytes: &[u8]) -> Passed {
        if self.blind {
            return Passed { bytes: bytes.to_vec(), messages: Vec::new() };
        }
        self.held.extend_from_slice(bytes);
        let mut messages = Vec::new();
        // (Whole frames up to here go now: nothing of a message held.)
        let mut pass = 0;
        loop {
            let head = match frame_head(&self.held[self.at..]) {
                Head::More => break,
                Head::Unreadable => return self.give_up(messages),
                Head::Frame { fin, rsv1, opcode, header, len, mask } => (fin, rsv1, opcode, header, len, mask),
            };
            let (fin, rsv1, opcode, header, len, mask) = head;
            let Some(end) = self.at.checked_add(header).and_then(|h| h.checked_add(len)) else { return self.give_up(messages) };
            if end > self.held.len() {
                if len > MESSAGE_MOST {
                    return self.give_up(messages);
                }
                break;
            }
            let mut payload = self.held[self.at + header..end].to_vec();
            if let Some(key) = mask {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= key[i % 4];
                }
            }
            match opcode {
                // A message begins (text, binary).
                0x1 | 0x2 => {
                    if self.message.is_some() {
                        return self.give_up(messages);
                    }
                    self.message = Some((opcode == 0x1, rsv1, payload));
                }
                // More of it.
                0x0 => match self.message.as_mut() {
                    Some((_, _, so_far)) if so_far.len() + payload.len() <= MESSAGE_MOST => so_far.extend_from_slice(&payload),
                    _ => return self.give_up(messages),
                },
                // Close, ping, pong: whole frames of their own.
                0x8..=0xA => {}
                _ => return self.give_up(messages),
            }
            self.at = end;
            if fin && opcode <= 0x2 {
                let Some((text, compressed, payload)) = self.message.take() else { return self.give_up(messages) };
                let data = if compressed {
                    let afresh = self.afresh;
                    match self.inflate.as_mut().and_then(|d| inflate(d, afresh, &payload)) {
                        Some(d) => d,
                        None => return self.give_up(messages),
                    }
                } else {
                    payload
                };
                if text {
                    messages.push(String::from_utf8_lossy(&data).into_owned());
                }
            }
            if self.message.is_none() {
                pass = self.at;
            }
        }
        let bytes: Vec<u8> = self.held.drain(..pass).collect();
        self.at -= pass;
        Passed { bytes, messages }
    }

    /// Stop reading: all that's held goes, with the messages read so far.
    fn give_up(&mut self, messages: Vec<String>) -> Passed {
        self.blind = true;
        self.message = None;
        self.at = 0;
        Passed { bytes: std::mem::take(&mut self.held), messages }
    }
}

enum Head {
    More,
    Unreadable,
    Frame { fin: bool, rsv1: bool, opcode: u8, header: usize, len: usize, mask: Option<[u8; 4]> },
}

/// A frame's head at the start of `b`: its flags, opcode, how long its head
/// and payload are, and its mask.
fn frame_head(b: &[u8]) -> Head {
    if b.len() < 2 {
        return Head::More;
    }
    let (fin, rsv1, opcode) = (b[0] & 0x80 != 0, b[0] & 0x40 != 0, b[0] & 0x0f);
    let masked = b[1] & 0x80 != 0;
    let (len, mut header) = match b[1] & 0x7f {
        126 => match b.get(2..4) {
            Some(n) => (u64::from(u16::from_be_bytes([n[0], n[1]])), 4),
            None => return Head::More,
        },
        127 => match b.get(2..10) {
            Some(n) => (u64::from_be_bytes(n.try_into().expect("eight bytes")), 10),
            None => return Head::More,
        },
        n => (u64::from(n), 2),
    };
    let Ok(len) = usize::try_from(len) else { return Head::Unreadable };
    let mask = if masked {
        let Some(k) = b.get(header..header + 4) else { return Head::More };
        header += 4;
        Some([k[0], k[1], k[2], k[3]])
    } else {
        None
    };
    Head::Frame { fin, rsv1, opcode, header, len, mask }
}

/// One message's payload as `permessage-deflate` compressed it, inflated
/// (its sync-flush tail put back), the context kept for the next unless
/// `afresh`.
fn inflate(d: &mut Decompress, afresh: bool, payload: &[u8]) -> Option<Vec<u8>> {
    if afresh {
        d.reset(false);
    }
    let mut input = payload.to_vec();
    input.extend_from_slice(&[0x00, 0x00, 0xff, 0xff]);
    let mut out: Vec<u8> = Vec::with_capacity(payload.len().saturating_mul(4).max(4096));
    let mut at = 0;
    loop {
        if out.capacity() - out.len() < 4096 {
            out.reserve(out.capacity().max(4096));
        }
        let (before_in, before_out) = (d.total_in(), d.total_out());
        let status = d.decompress_vec(&input[at..], &mut out, FlushDecompress::Sync).ok()?;
        at += usize::try_from(d.total_in() - before_in).ok()?;
        if status == Status::StreamEnd {
            // (A last block: what follows starts a stream of its own.)
            d.reset(false);
            return Some(out);
        }
        if at >= input.len() && out.len() < out.capacity() {
            return Some(out);
        }
        if (d.total_in(), d.total_out()) == (before_in, before_out) || out.len() > MESSAGE_MOST {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    /// A server's frame (unmasked).
    fn frame(fin: bool, rsv1: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut f = vec![(if fin { 0x80 } else { 0 }) | (if rsv1 { 0x40 } else { 0 }) | opcode];
        match payload.len() {
            n if n < 126 => f.push(n as u8),
            n if n <= 0xffff => {
                f.push(126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(127);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        f.extend_from_slice(payload);
        f
    }

    /// A message compressed as a server keeping its context does: a sync
    /// flush, its tail taken off.
    fn deflated(c: &mut Compress, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len() + 64);
        c.compress_vec(text.as_bytes(), &mut out, FlushCompress::Sync).unwrap();
        assert!(out.ends_with(&[0x00, 0x00, 0xff, 0xff]));
        out.truncate(out.len() - 4);
        out
    }

    /// What's passed on is what came, byte for byte, however it's split;
    /// a message's frames held only until it's whole; each text message
    /// read; pings passed on with what they came between.
    #[test]
    fn passed_on_as_it_came_each_message_read() {
        let mut wire = frame(true, false, 0x1, br#"{"type":"response.created"}"#);
        wire.extend(frame(true, false, 0x9, b"hi"));
        wire.extend(frame(false, false, 0x1, br#"{"type":"response."#));
        wire.extend(frame(true, false, 0x9, b""));
        wire.extend(frame(true, false, 0x0, br#"completed","response":{"usage":{"output_tokens":3}}}"#));
        let long = format!(r#"{{"type":"x","pad":"{}"}}"#, "a".repeat(70_000));
        wire.extend(frame(true, false, 0x1, long.as_bytes()));
        for step in [1, 3, 7, 1000, wire.len()] {
            let mut r = Reader::new(None);
            let (mut passed, mut messages) = (Vec::new(), Vec::new());
            for chunk in wire.chunks(step) {
                let p = r.push(chunk);
                passed.extend(p.bytes);
                messages.extend(p.messages);
            }
            assert_eq!(passed, wire, "split every {step}");
            assert_eq!(messages.len(), 3);
            assert_eq!(messages[1], r#"{"type":"response.completed","response":{"usage":{"output_tokens":3}}}"#);
            assert_eq!(messages[2].len(), long.len());
            assert!(!r.blind());
        }
        // A message's first frame alone is held back.
        let mut r = Reader::new(None);
        let first = frame(false, false, 0x1, b"{");
        assert_eq!(r.push(&first), Passed::default());
        assert_eq!(r.push(&frame(true, false, 0x0, b"}")).messages, ["{}"]);
    }

    /// Compressed messages (`permessage-deflate`, the server keeping its
    /// context from one to the next) are read as they say, and passed on
    /// as they came.
    #[test]
    fn compressed_messages_read_with_their_context() {
        let mut c = Compress::new(Compression::default(), false);
        let texts = [
            r#"{"type":"response.created","response":{"model":"gpt-5"}}"#,
            r#"{"type":"response.created","response":{"model":"gpt-5"}}"#,
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":5,"output_tokens":2}}}"#,
        ];
        let mut wire = Vec::new();
        for t in texts {
            wire.extend(frame(true, true, 0x1, &deflated(&mut c, t)));
        }
        let mut r = Reader::new(Some("permessage-deflate; client_max_window_bits=15"));
        let mut got = Passed::default();
        for chunk in wire.chunks(5) {
            let p = r.push(chunk);
            got.bytes.extend(p.bytes);
            got.messages.extend(p.messages);
        }
        assert_eq!(got.bytes, wire);
        assert_eq!(got.messages, texts);
        // Without its context (each message afresh) as the server said.
        let mut c = Compress::new(Compression::default(), false);
        let mut r = Reader::new(Some("permessage-deflate; server_no_context_takeover"));
        for t in texts {
            c.reset();
            let m = deflated(&mut c, t);
            assert_eq!(r.push(&frame(true, true, 0x1, &m)).messages, [t]);
        }
    }

    /// What reading a socket costs: a turn as ChatGPT sends one (its
    /// meters, created, 300 deltas, completed), compressed with its
    /// context kept, read in 16 KB pieces. Run on a release build:
    /// `cargo test --release -p olo-token-pool --lib socket::tests::reading_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reading_cost() {
        let mut c = Compress::new(Compression::default(), false);
        let mut texts = vec![r#"{"type":"codex.rate_limits","plan_type":"pro","rate_limits":{"allowed":true,"limit_reached":false,"primary":{"used_percent":42.0,"window_minutes":300,"reset_after_seconds":3600,"reset_at":1795356000},"secondary":null}}"#.to_owned()];
        texts.push(r#"{"type":"response.created","response":{"id":"resp_1","model":"gpt-6.2-astra"}}"#.to_owned());
        for i in 0..300 {
            texts.push(format!(r#"{{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"word{i} ","sequence_number":{i}}}"#));
        }
        texts.push(r#"{"type":"response.completed","response":{"usage":{"input_tokens":12000,"output_tokens":600}}}"#.to_owned());
        let mut wire = Vec::new();
        for t in &texts {
            wire.extend(frame(true, true, 0x1, &deflated(&mut c, t)));
        }
        const TURNS: u32 = 2000;
        let t = std::time::Instant::now();
        let mut read = 0;
        for _ in 0..TURNS {
            // (Each turn a socket of its own: its context starts afresh.)
            let mut r = Reader::new(Some("permessage-deflate"));
            for chunk in wire.chunks(16 << 10) {
                read += r.push(chunk).messages.len();
            }
        }
        let each = t.elapsed().as_secs_f64() / f64::from(TURNS);
        assert_eq!(read, texts.len() * TURNS as usize);
        println!("a turn of {} messages ({} bytes on the wire): {:.0} µs to read, {:.2} µs a message", texts.len(), wire.len(), each * 1e6, each * 1e6 / texts.len() as f64);
    }

    /// What it can't read goes on as it came, all of it, read no further.
    #[test]
    fn what_it_cant_read_goes_on_unread() {
        let mut r = Reader::new(None);
        let mut wire = frame(false, false, 0x1, b"{");
        wire.extend(frame(true, false, 0x3, b"?"));
        wire.extend(frame(true, false, 0x1, b"{}"));
        let p = r.push(&wire);
        assert_eq!(p.bytes, wire);
        assert!(r.blind());
        assert_eq!(r.push(b"more").bytes, b"more");
    }
}
