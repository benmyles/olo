//! A WebSocket's frames read as they pass (RFC 6455), each message whole:
//! what the server says on a Codex socket the pool passes on (each turn's
//! start and end, its usage, a 429), its bytes passed on as they came. A
//! message's frames are held only until it's whole (a moment: Codex's
//! events are small, one frame each), and one compressed by
//! `permessage-deflate` (RFC 7692) is inflated for the pool's own reading,
//! the server's context kept from message to message as the client keeps
//! it. A socket it can't read (a frame it doesn't know, a message past
//! `MESSAGE_MOST`) goes on as it comes, read no further.

use std::collections::VecDeque;

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
    inflate: Option<Inflater>,
    afresh: bool,
    /// Read no further: everything passed on as it comes.
    blind: bool,
    pub(crate) closed: bool,
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
            inflate: deflate.map(|_| Inflater { decoder: Decompress::new(false), history: VecDeque::new() }),
            afresh: deflate.is_some_and(|d| d.split(';').any(|p| p.trim().eq_ignore_ascii_case("server_no_context_takeover"))),
            closed: false,
            blind: extensions.is_some_and(|e| e.split(',').any(|item| !item.split(';').next().unwrap_or_default().trim().eq_ignore_ascii_case("permessage-deflate"))),
        }
    }

    /// Whether it has stopped reading (what it passes on is then unread).
    pub fn blind(&self) -> bool {
        self.blind
    }

    /// `bytes` came from the server: what to pass on now, and the
    /// messages that completes.
    pub fn push(&mut self, bytes: &[u8]) -> Passed {
        let mut messages = Vec::new();
        let (bytes, _) = self.read_while(bytes, |text| {
            messages.push(text.to_owned());
            true
        });
        Passed { bytes, messages }
    }

    /// Return the original bytes up to the first complete text message
    /// rejected by the observer. Do not inspect anything after it.
    pub(crate) fn read_while(&mut self, bytes: &[u8], mut accept: impl FnMut(&str) -> bool) -> (Vec<u8>, bool) {
        if self.blind {
            return (bytes.to_vec(), false);
        }
        self.held.extend_from_slice(bytes);
        // First byte still held for the message, relative to this return.
        let mut message_start = 0;
        // (Whole frames up to here go now: nothing of a message held.)
        let mut pass = 0;
        loop {
            let head = match frame_head(&self.held[self.at..]) {
                Head::More => break,
                Head::Unreadable => return self.give_up(),
                Head::Frame { fin, rsv1, opcode, header, len, mask } => (fin, rsv1, opcode, header, len, mask),
            };
            let (fin, rsv1, opcode, header, len, mask) = head;
            let Some(end) = self.at.checked_add(header).and_then(|h| h.checked_add(len)) else { return self.give_up() };
            if len > MESSAGE_MOST || end - pass > MESSAGE_MOST + 14 {
                return self.give_up();
            }
            if end > self.held.len() {
                break;
            }
            if mask.is_some() || (rsv1 && (self.inflate.is_none() || !matches!(opcode, 1 | 2))) {
                return self.give_up();
            }
            let payload = self.held[self.at + header..end].to_vec();
            match opcode {
                // A message begins (text, binary).
                0x1 | 0x2 => {
                    if self.message.is_some() {
                        return self.give_up();
                    }
                    message_start = self.at;
                    self.message = Some((opcode == 0x1, rsv1, payload));
                }
                // More of it.
                0x0 => match self.message.as_mut() {
                    Some((_, _, so_far)) if so_far.len() + payload.len() <= MESSAGE_MOST => so_far.extend_from_slice(&payload),
                    _ => return self.give_up(),
                },
                // Close, ping, pong: whole frames of their own.
                0x8..=0xA if fin && len <= 125 => {
                    // Deliver a ping/close in order, including any fragments
                    // before it. A client only sees a data message at FIN.
                    self.closed |= opcode == 8;
                    pass = end;
                    message_start = end;
                }
                _ => return self.give_up(),
            }
            self.at = end;
            if fin && opcode <= 0x2 {
                let Some((text, compressed, payload)) = self.message.take() else { return self.give_up() };
                let data = if compressed {
                    let afresh = self.afresh;
                    match self.inflate.as_mut().and_then(|d| d.message(afresh, &payload)) {
                        Some(d) => d,
                        None => return self.give_up(),
                    }
                } else {
                    payload
                };
                if text {
                    let Ok(text) = std::str::from_utf8(&data) else { return self.give_up() };
                    if !accept(text) {
                        let mut prefix = std::mem::take(&mut self.held);
                        prefix.truncate(message_start);
                        self.at = 0;
                        self.blind = true;
                        return (prefix, true);
                    }
                }
            }
            if self.message.is_none() {
                pass = self.at;
            }
        }
        let bytes: Vec<u8> = self.held.drain(..pass).collect();
        self.at -= pass;
        (bytes, false)
    }

    /// Bytes still held when the transport ends inside a frame or message.
    pub(crate) fn finish(&mut self) -> Vec<u8> {
        self.give_up().0
    }

    fn give_up(&mut self) -> (Vec<u8>, bool) {
        self.blind = true;
        self.message = None;
        self.at = 0;
        (std::mem::take(&mut self.held), false)
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
    if b[0] & 0x30 != 0 {
        return Head::Unreadable;
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
    if matches!(b[1] & 0x7f, 126) && len < 126 || matches!(b[1] & 0x7f, 127) && !(65536..=i64::MAX as u64).contains(&len) {
        return Head::Unreadable;
    }
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

/// Recognize the start of a client data message and an entire close
/// frame without copying or decompressing its application payload.
#[derive(Default)]
pub(crate) struct ClientFrames {
    header: [u8; 14],
    used: usize,
    remaining: usize,
    closing: bool,
}

impl ClientFrames {
    /// Number of bytes to relay, and whether a close frame finished.
    pub(crate) fn before_data(&mut self, input: &[u8], mut allow: impl FnMut() -> bool) -> (usize, bool) {
        let mut at = 0;
        while at < input.len() {
            if self.remaining != 0 {
                let n = self.remaining.min(input.len() - at);
                self.remaining -= n;
                at += n;
            } else {
                if self.used == 0 && matches!(input[at] & 15, 1 | 2) && !allow() {
                    return (at, false);
                }
                self.header[self.used] = input[at];
                self.used += 1;
                at += 1;
                match frame_head(&self.header[..self.used]) {
                    Head::More => continue,
                    Head::Unreadable => return (at - 1, false),
                    Head::Frame { opcode, len, .. } => {
                        self.remaining = len;
                        self.used = 0;
                        self.closing = opcode == 8;
                    }
                }
            }
            if self.remaining == 0 && self.closing {
                return (at, true);
            }
        }
        (at, false)
    }
}

/// RFC 7692 permits more blocks after a byte-aligned final block. Keep
/// the history needed to resume raw DEFLATE at that boundary.
struct Inflater {
    decoder: Decompress,
    history: VecDeque<u8>,
}

impl Inflater {
    fn message(&mut self, afresh: bool, payload: &[u8]) -> Option<Vec<u8>> {
        const WINDOW: usize = 32768;
        if afresh {
            self.decoder.reset(false);
            self.history.clear();
        }
        let mut input = payload.to_vec();
        input.extend_from_slice(&[0, 0, 0xff, 0xff]);
        let mut out = Vec::new();
        let mut at = 0;
        let mut block = [0; 8192];
        loop {
            let (before_in, before_out) = (self.decoder.total_in(), self.decoder.total_out());
            let status = self.decoder.decompress(&input[at..], &mut block, FlushDecompress::Sync).ok()?;
            let used = usize::try_from(self.decoder.total_in() - before_in).ok()?;
            let wrote = usize::try_from(self.decoder.total_out() - before_out).ok()?;
            if wrote > MESSAGE_MOST - out.len() {
                return None;
            }
            out.extend_from_slice(&block[..wrote]);
            let discard = (self.history.len() + wrote).saturating_sub(WINDOW);
            self.history.drain(..discard);
            self.history.extend(&block[..wrote]);
            at += used;
            if status == Status::StreamEnd {
                self.decoder.reset(false);
                if !self.history.is_empty() {
                    self.decoder.set_dictionary(self.history.make_contiguous()).ok()?;
                }
            }
            if at == input.len() && (wrote < block.len() || status == Status::StreamEnd) {
                return Some(out);
            }
            if used == 0 && wrote == 0 {
                return None;
            }
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

    /// A ping between a message's frames goes on at once, with the frames
    /// before it (Codex's fourth review: it was held with the message, and
    /// a server waiting for its pong before the message's end waited for
    /// ever); the message is still read whole.
    #[test]
    fn a_ping_inside_a_message_goes_at_once() {
        let mut r = Reader::new(None);
        let (first, ping, last) = (frame(false, false, 0x1, b"one, "), frame(true, false, 0x9, b"p"), frame(true, false, 0x0, b"two"));
        let a = r.push(&first);
        let b = r.push(&ping);
        let c = r.push(&last);
        assert!(a.bytes.is_empty());
        assert_eq!(b.bytes, [&first[..], &ping[..]].concat());
        assert_eq!((c.bytes, c.messages), (last, vec!["one, two".to_owned()]));
        assert!(!r.blind());
    }

    /// A compressed message may end a DEFLATE stream and go on with
    /// another (RFC 7692 allows it; one taking the context over may refer
    /// to what came before): read whole, and the next message too (it
    /// was cut at the first's end, and the next unreadable).
    #[test]
    fn a_message_of_two_streams_read_whole() {
        let mut first = Vec::with_capacity(64);
        Compress::new(Compression::fast(), false).compress_vec(b"Hello", &mut first, FlushCompress::Finish).unwrap();
        let mut second = Compress::new(Compression::fast(), false);
        let mut wire_payload = first.clone();
        wire_payload.extend(deflated(&mut second, " world"));
        let mut r = Reader::new(Some("permessage-deflate"));
        let p = r.push(&frame(true, true, 0x1, &wire_payload));
        assert_eq!(p.messages, ["Hello world"]);

        // A stream ended, the next message's referring back past it.
        let plain = "the model reports its response";
        let mut ended = Vec::with_capacity(128);
        Compress::new(Compression::default(), false).compress_vec(plain.as_bytes(), &mut ended, FlushCompress::Finish).unwrap();
        // (An empty stored block after it, as RFC 7692 has a sender add,
        // less the four bytes taken off every message: its first byte.)
        ended.push(0);
        let mut r = Reader::new(Some("permessage-deflate"));
        assert_eq!(r.push(&frame(true, true, 0x1, &ended)).messages, [plain]);
        let mut next = Compress::new(Compression::default(), false);
        next.set_dictionary(plain.as_bytes()).unwrap();
        let again = deflated(&mut next, plain);
        assert_eq!(r.push(&frame(true, true, 0x1, &again)).messages, [plain]);
        assert!(!r.blind());
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
