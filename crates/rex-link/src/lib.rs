//! Olo's helper on a Rex host (Ben, 2026-11-22: "if it's helpful to have
//! e.g. a daemon server side to help for this too you can also add that
//! and have the rex app setup install that too when it manages the rex
//! server … i just want this to be the best ever implementation").
//!
//! The Rex server takes two requests that reach it together at once,
//! and may write what they type into a terminal out of order (HACKING,
//! Rex, "A slow link, measured": forty letters came back shuffled). So
//! the app sends one write at a time and waits for its answer before
//! the next, and a letter typed while another is on its way costs a
//! round trip and a half of the link.
//!
//! This program runs on the host, between the app's control connection
//! (the SSH channel that was `rex __internal dial-stdio`) and the
//! server. Everything passes through it as it comes, byte for byte,
//! except that a request that types into a terminal is held while the
//! one before it is unanswered: the waiting happens here, where an
//! answer takes a fraction of a millisecond, and the app sends each key
//! as it is typed. One round trip a key, in order.
//!
//! It knows only as much of the connection as that takes: where the
//! HTTP upgrade ends, where each WebSocket frame ends, and of a text
//! frame whether it is one of those requests or an answer ([`Scanner`],
//! [`Gate`]). It tells the app it is there by a header in the server's
//! answer to the upgrade ([`HERE`]). Nothing is altered otherwise, and
//! what it doesn't understand passes untouched.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// What this helper does, counted: the app reads it from [`HEADER`] and
/// asks of the helper only what its number says it does. 1: what is
/// typed is kept in order.
pub const VERSION: u32 = 1;

/// The name of the header the helper adds to the server's answer to
/// the upgrade, by which the app knows its control connection goes
/// through it, and what the helper does ([`VERSION`]).
pub const HEADER: &str = "X-Olo-Rex-Link";

/// That header, whole.
pub const HERE: &str = "X-Olo-Rex-Link: 1";

/// The requests that type into a terminal: given to the server one at
/// a time.
pub const TYPED: [&str; 2] = ["com.superlogical.terminal.write", "client.key"];

/// The longest a request is held for another's answer: as long as the
/// app itself waits for an answer when it sends one at a time (its
/// request timeout), so a server that is slow to take what is typed
/// (a program that isn't reading, a server stopped) gets it in order
/// for exactly as long as it did before, and one that never answers
/// doesn't stop what is typed for good.
pub const HELD_AT_MOST: Duration = Duration::from_secs(30);

/// The largest frame kept whole to be looked into. A larger one passes
/// as it comes, unread (no request that types is so large: the server
/// takes none; and no answer to one).
pub const LARGEST: usize = 16 << 20;

/// A piece of one direction of the connection, as it passes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// Bytes passed as they are: part of a frame too large to keep, or
    /// of a connection that isn't an upgraded one.
    Raw(Vec<u8>),
    /// The HTTP head (the upgrade's request, or its answer).
    Head(Vec<u8>),
    /// One WebSocket frame, whole, as it was; and where it is a text
    /// message by itself, what it says (unmasked).
    Frame { bytes: Vec<u8>, text: Option<Vec<u8>> },
}

impl Piece {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Piece::Raw(b) | Piece::Head(b) | Piece::Frame { bytes: b, .. } => b,
        }
    }
}

/// One direction of the connection cut into pieces: the HTTP head,
/// then frames. Every byte given to it comes out again, in order, in
/// one piece or another: by [`Scanner::push`] once the piece is whole,
/// and at the connection's end by [`Scanner::rest`].
#[derive(Default)]
pub struct Scanner {
    buf: Vec<u8>,
    past_head: bool,
    /// Not frames: everything passes as it comes ([`Scanner::plain`]).
    plain: bool,
    /// So many bytes more of a frame too large to keep pass as they come.
    passing: u64,
}

impl Scanner {
    pub fn new() -> Scanner {
        Scanner::default()
    }

    /// What follows isn't frames (the upgrade was refused: the head is
    /// followed by a page, not a connection): everything passes as it
    /// comes from here.
    pub fn plain(&mut self) -> Option<Piece> {
        self.plain = true;
        self.rest()
    }

    /// What is kept, at the connection's end: the beginning of a piece
    /// that never became whole.
    pub fn rest(&mut self) -> Option<Piece> {
        (!self.buf.is_empty()).then(|| Piece::Raw(std::mem::take(&mut self.buf)))
    }

    /// More bytes of the direction: the pieces now whole.
    pub fn push(&mut self, mut bytes: &[u8]) -> Vec<Piece> {
        let mut out = Vec::new();
        // (Nothing is kept of what only passes.)
        if self.buf.is_empty() && (self.plain || self.passing > 0) {
            let n = if self.plain { bytes.len() } else { bytes.len().min(self.passing as usize) };
            if n > 0 {
                out.push(Piece::Raw(bytes[..n].to_vec()));
            }
            if !self.plain {
                self.passing -= n as u64;
            }
            bytes = &bytes[n..];
            if bytes.is_empty() {
                return out;
            }
        }
        self.buf.extend_from_slice(bytes);
        loop {
            if self.plain {
                out.extend(self.rest());
                break;
            }
            if self.passing > 0 {
                let n = self.buf.len().min(self.passing as usize);
                if n == 0 {
                    break;
                }
                out.push(Piece::Raw(self.buf.drain(..n).collect()));
                self.passing -= n as u64;
                continue;
            }
            if !self.past_head {
                let Some(end) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") else { break };
                out.push(Piece::Head(self.buf.drain(..end + 4).collect()));
                self.past_head = true;
                continue;
            }
            if self.buf.len() < 2 {
                break;
            }
            let (first, second) = (self.buf[0], self.buf[1]);
            let masked = second & 0x80 != 0;
            let (len, mut at) = match second & 0x7f {
                126 if self.buf.len() >= 4 => (u64::from(u16::from_be_bytes([self.buf[2], self.buf[3]])), 4),
                127 if self.buf.len() >= 10 => (u64::from_be_bytes(self.buf[2..10].try_into().unwrap()), 10),
                126 | 127 => break,
                n => (u64::from(n), 2),
            };
            if masked {
                at += 4;
            }
            // Too large to keep: its header and what has come pass now,
            // the rest as it comes.
            if len > LARGEST as u64 {
                if self.buf.len() < at {
                    break;
                }
                self.passing = (at as u64).saturating_add(len);
                continue;
            }
            let end = at + len as usize;
            if self.buf.len() < end {
                break;
            }
            // A text message in one frame (FIN, opcode 1, no extension's
            // bit): what it says.
            let text = (first == 0x81).then(|| {
                let mut text = self.buf[at..end].to_vec();
                if masked {
                    let key = [self.buf[at - 4], self.buf[at - 3], self.buf[at - 2], self.buf[at - 1]];
                    text.iter_mut().enumerate().for_each(|(i, b)| *b ^= key[i % 4]);
                }
                text
            });
            out.push(Piece::Frame { bytes: self.buf.drain(..end).collect(), text });
        }
        out
    }
}

/// Whether a head is the server's yes to the upgrade.
pub fn upgraded(head: &[u8]) -> bool {
    head.starts_with(b"HTTP/1.1 101") && head.ends_with(b"\r\n\r\n")
}

/// The server's answer to the upgrade with the helper's header in it
/// (where it is a yes: anything else is left alone).
pub fn announced(head: &[u8]) -> Vec<u8> {
    if !upgraded(head) {
        return head.to_vec();
    }
    let mut out = head[..head.len() - 2].to_vec();
    out.extend_from_slice(HERE.as_bytes());
    out.extend_from_slice(b"\r\n\r\n");
    out
}

/// As much of a message as says what it is: nothing else of it is
/// kept, however large it is.
#[derive(serde::Deserialize)]
struct Top<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<std::borrow::Cow<'a, str>>,
    #[serde(borrow)]
    method: Option<std::borrow::Cow<'a, str>>,
    id: Option<Id<'a>>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Id<'a> {
    Text(#[serde(borrow)] std::borrow::Cow<'a, str>),
    Number(u64),
}

impl Id<'_> {
    fn said(&self) -> String {
        match self {
            Id::Text(id) => id.to_string(),
            Id::Number(id) => id.to_string(),
        }
    }
}

/// The id of the request a text message is, if it is one that types
/// into a terminal.
fn typed(text: &[u8]) -> Option<String> {
    // (Looked at only if it names one: most messages don't.)
    if !TYPED.iter().any(|method| contains(text, method.as_bytes())) {
        return None;
    }
    let top: Top = serde_json::from_slice(text).ok()?;
    (top.kind.as_deref() == Some("request") && top.method.as_deref().is_some_and(|m| TYPED.contains(&m))).then(|| top.id.map(|id| id.said())).flatten()
}

/// Whether a text message is the answer to the request `id`.
fn answers(text: &[u8], id: &str) -> bool {
    // (Looked at only if it holds the id and the word: most don't.)
    if !contains(text, id.as_bytes()) || !contains(text, b"\"response\"") {
        return false;
    }
    serde_json::from_slice::<Top>(text).is_ok_and(|top| top.kind.as_deref() == Some("response") && top.id.is_some_and(|got| got.said() == id))
}

fn contains(text: &[u8], what: &[u8]) -> bool {
    text.windows(what.len()).any(|w| w == what)
}

/// What goes on to the server, and when: everything in the order it
/// came, a request that types into a terminal only once the one before
/// it is answered.
pub struct Gate {
    /// The typed request the server hasn't answered: its id, and since
    /// when.
    waiting: Option<(String, Instant)>,
    /// What came from the app behind it.
    held: VecDeque<Piece>,
    /// Nothing is held any more ([`Gate::open`]).
    open: bool,
    /// The longest a request is held for another's answer.
    at_most: Duration,
}

impl Default for Gate {
    fn default() -> Self {
        Gate::new()
    }
}

impl Gate {
    pub fn new() -> Gate {
        Gate::holding(HELD_AT_MOST)
    }

    /// A gate that holds a request for another's answer `at_most` so
    /// long ([`HELD_AT_MOST`], but for a test of it).
    pub fn holding(at_most: Duration) -> Gate {
        Gate { waiting: None, held: VecDeque::new(), open: false, at_most }
    }

    /// A piece from the app: what goes on to the server now, in order.
    pub fn from_app(&mut self, piece: Piece, now: Instant) -> Vec<Vec<u8>> {
        self.held.push_back(piece);
        self.pass(now)
    }

    /// A piece from the server (it goes on to the app whatever it is):
    /// what an answer in it lets go on to the server.
    pub fn from_server(&mut self, piece: &Piece, now: Instant) -> Vec<Vec<u8>> {
        if let (Piece::Frame { text: Some(text), .. }, Some((id, _))) = (piece, &self.waiting)
            && answers(text, id)
        {
            self.waiting = None;
            return self.pass(now);
        }
        Vec::new()
    }

    /// The server's side isn't a connection whose answers can be read
    /// (the upgrade was refused): nothing is held from here, and what
    /// was goes on.
    pub fn open(&mut self, now: Instant) -> Vec<Vec<u8>> {
        self.open = true;
        self.waiting = None;
        self.pass(now)
    }

    /// What time lets go: when a request's answer hasn't come in
    /// [`HELD_AT_MOST`], everything held behind it goes on together
    /// (nothing typed is kept longer than that), and the last of it is
    /// waited for in its turn.
    pub fn tick(&mut self, now: Instant) -> Vec<Vec<u8>> {
        if !self.waiting.as_ref().is_some_and(|(_, since)| now.saturating_duration_since(*since) >= self.at_most) {
            return Vec::new();
        }
        let mut out = Vec::new();
        while let Some(piece) = self.held.pop_front() {
            if let Piece::Frame { text: Some(text), .. } = &piece
                && let Some(id) = typed(text)
            {
                self.waiting = Some((id, now));
            }
            out.push(piece.bytes().to_vec());
        }
        if out.is_empty() {
            // (Nothing waits behind it: it is waited for no longer.)
            self.waiting = None;
        }
        out
    }

    /// When [`Self::tick`] next has something to do, if something is
    /// held.
    pub fn deadline(&self) -> Option<Instant> {
        self.waiting.as_ref().filter(|_| !self.held.is_empty()).map(|(_, since)| *since + self.at_most)
    }

    fn pass(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(piece) = self.held.front() {
            let typed = match piece {
                Piece::Frame { text: Some(text), .. } if !self.open => typed(text),
                _ => None,
            };
            if typed.is_some() && self.waiting.is_some() {
                break;
            }
            if let Some(id) = typed {
                self.waiting = Some((id, now));
            }
            out.push(self.held.pop_front().unwrap().bytes().to_vec());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text frame as a client sends it (masked) or a server (not).
    fn frame(text: &str, mask: Option<[u8; 4]>) -> Vec<u8> {
        let payload = text.as_bytes();
        let mut out = vec![0x81];
        let masked = if mask.is_some() { 0x80 } else { 0 };
        match payload.len() {
            n if n < 126 => out.push(masked | n as u8),
            n if n < 65536 => {
                out.push(masked | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(masked | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        match mask {
            Some(key) => {
                out.extend_from_slice(&key);
                out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
            }
            None => out.extend_from_slice(payload),
        }
        out
    }

    fn write(id: u32, data: &str) -> String {
        format!(r#"{{"type":"request","id":"{id}","method":"com.superlogical.terminal.write","payload":{{"session_id":"s","block_id":"b","args":{{"data":"{data}"}}}}}}"#)
    }

    fn answer(id: u32) -> String {
        format!(r#"{{"type":"response","id":"{id}","ok":true,"seq":7}}"#)
    }

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, below: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % below.max(1)
        }
    }

    #[test]
    fn a_connection_is_cut_into_its_head_and_its_frames_wherever_the_bytes_break() {
        let head = b"GET / HTTP/1.1\r\nUpgrade: websocket\r\n\r\n".to_vec();
        let long = "x".repeat(70_000);
        let texts = [write(1, "a"), "{}".to_owned(), "y".repeat(300), long.clone(), write(2, "b")];
        let mut all = head.clone();
        let mut frames = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            let f = frame(text, Some([i as u8, 7, 99, 200]));
            all.extend_from_slice(&f);
            frames.push(f);
        }
        // A ping, a binary frame and the first frame of a message in
        // parts: frames, with nothing to say.
        let odd: [&[u8]; 3] = [&[0x89, 0x00], &[0x82, 0x02, 1, 2], &[0x01, 0x03, b'a', b'b', b'c']];
        for f in odd {
            all.extend_from_slice(f);
            frames.push(f.to_vec());
        }
        let mut rng = Lcg(5);
        for _ in 0..200 {
            let mut scanner = Scanner::new();
            let mut pieces = Vec::new();
            let mut at = 0;
            while at < all.len() {
                let most = if rng.next(4) == 0 { 40_000 } else { 9 };
                let n = (1 + rng.next(most) as usize).min(all.len() - at);
                pieces.extend(scanner.push(&all[at..at + n]));
                at += n;
            }
            assert_eq!(pieces[0], Piece::Head(head.clone()));
            assert_eq!(pieces.len(), 1 + frames.len());
            for (i, piece) in pieces[1..].iter().enumerate() {
                let Piece::Frame { bytes, text } = piece else { panic!("{piece:?}") };
                assert_eq!(bytes, &frames[i], "frame {i} is as it was");
                assert_eq!(text.as_deref(), texts.get(i).map(|t| t.as_bytes()), "frame {i}");
            }
        }
    }

    /// Every byte in comes out again, in order, whatever it is: frames
    /// of an extension this doesn't read, frames too large to keep
    /// (passed as they come, not kept), a refused upgrade's page, a
    /// piece cut off by the connection's end.
    #[test]
    fn everything_given_comes_out_again_in_order_whatever_it_is() {
        // A frame with an extension's bit: a frame, with nothing to say.
        let mut scanner = Scanner::new();
        let mut out = scanner.push(b"HTTP/1.1 101 Switching Protocols\r\n\r\n");
        out.extend(scanner.push(&[0xC1, 0x05, 1, 2, 3]));
        out.extend(scanner.push(&[4, 5, 0x81, 0x02, b'{', b'}']));
        assert_eq!(out.len(), 3);
        assert_eq!(out[1], Piece::Frame { bytes: vec![0xC1, 0x05, 1, 2, 3, 4, 5], text: None });
        assert_eq!(out[2], Piece::Frame { bytes: vec![0x81, 0x02, b'{', b'}'], text: Some(b"{}".to_vec()) });

        // A frame too large to keep passes as it comes, and the frame
        // after it is read again.
        let len = LARGEST + 1000;
        let mut big = vec![0x82, 127];
        big.extend_from_slice(&(len as u64).to_be_bytes());
        big.extend(std::iter::repeat_n(7u8, len));
        let after = frame(&write(9, "z"), None);
        let mut all = big.clone();
        all.extend_from_slice(&after);
        let mut scanner = Scanner { past_head: true, ..Scanner::new() };
        let (mut got, mut kept_most, mut last) = (Vec::new(), 0, None);
        for chunk in all.chunks(50_000) {
            for piece in scanner.push(chunk) {
                got.extend_from_slice(piece.bytes());
                last = Some(piece);
            }
            kept_most = kept_most.max(scanner.buf.len());
        }
        assert_eq!(got.len(), all.len());
        assert!(got == all, "the bytes are as they were");
        assert!(kept_most < 60_000, "nothing of it was kept: {kept_most}");
        assert_eq!(last, Some(Piece::Frame { bytes: after, text: Some(write(9, "z").into_bytes()) }));

        // A refused upgrade: the head, then its page as it comes.
        let mut scanner = Scanner::new();
        let mut out = scanner.push(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\n\r\nnot ");
        assert!(matches!(&out[0], Piece::Head(head) if !upgraded(head)));
        out.extend(scanner.plain());
        out.extend(scanner.push(b"yours"));
        let said: Vec<u8> = out[1..].iter().flat_map(|p| p.bytes().to_vec()).collect();
        assert_eq!(said, b"not yours");
        assert!(out[1..].iter().all(|p| matches!(p, Piece::Raw(_))));

        // Cut off by the connection's end: what had come is given up.
        let mut scanner = Scanner { past_head: true, ..Scanner::new() };
        assert!(scanner.push(&[0x81, 0x05, b'a', b'b']).is_empty());
        assert_eq!(scanner.rest(), Some(Piece::Raw(vec![0x81, 0x05, b'a', b'b'])));
        assert_eq!(scanner.rest(), None);
    }

    /// Bytes of any kind, cut anywhere: all come out again, in order.
    #[test]
    fn any_bytes_cut_anywhere_come_out_whole() {
        let mut rng = Lcg(21);
        for round in 0..400 {
            let mut all = Vec::new();
            if round % 2 == 0 {
                all.extend_from_slice(b"GET / HTTP/1.1\r\n\r\n");
            }
            for _ in 0..rng.next(12) {
                match rng.next(4) {
                    // (Frames, some masked, some with odd first bytes.)
                    0 => all.extend(frame(&write(rng.next(99) as u32, "k"), (rng.next(2) == 0).then_some([1, 2, 3, 4]))),
                    1 => {
                        let n = rng.next(300) as usize;
                        all.push(rng.next(256) as u8);
                        all.push(n.min(125) as u8);
                        all.extend((0..n.min(125)).map(|i| i as u8));
                    }
                    // (And noise.)
                    _ => all.extend((0..rng.next(40)).map(|_| rng.next(256) as u8)),
                }
            }
            let mut scanner = Scanner::new();
            let mut got = Vec::new();
            let mut at = 0;
            while at < all.len() {
                let n = (1 + rng.next(17) as usize).min(all.len() - at);
                scanner.push(&all[at..at + n]).iter().for_each(|p| got.extend_from_slice(p.bytes()));
                at += n;
            }
            scanner.rest().iter().for_each(|p| got.extend_from_slice(p.bytes()));
            assert!(got == all, "round {round}");
        }
    }

    #[test]
    fn the_helper_says_it_is_there_in_the_upgrade_s_answer_and_nowhere_else() {
        assert_eq!(HERE, format!("{HEADER}: {VERSION}"));
        let head = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: abc\r\n\r\n";
        let said = announced(head);
        assert_eq!(String::from_utf8(said).unwrap(), "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: abc\r\nX-Olo-Rex-Link: 1\r\n\r\n");
        let refused = b"HTTP/1.1 403 Forbidden\r\n\r\n";
        assert_eq!(announced(refused), refused);
    }

    #[test]
    fn what_is_typed_goes_on_one_at_a_time_and_everything_in_order() {
        let t = Instant::now();
        let mut gate = Gate::new();
        let piece = |text: &str| Piece::Frame { bytes: frame(text, Some([1, 2, 3, 4])), text: Some(text.as_bytes().to_vec()) };
        let from_server = |text: &str| Piece::Frame { bytes: frame(text, None), text: Some(text.as_bytes().to_vec()) };
        // The first passes at once; the second, and what comes behind
        // it, wait for the first's answer.
        assert_eq!(gate.from_app(piece(&write(1, "a")), t).len(), 1);
        assert_eq!(gate.from_app(piece(&write(2, "b")), t).len(), 0);
        let resize = r#"{"type":"request","id":"3","method":"com.superlogical.terminal.resize","payload":{}}"#;
        assert_eq!(gate.from_app(piece(resize), t).len(), 0, "behind what is held: the order is the app's");
        assert_eq!(gate.from_app(piece(&write(4, "c")), t).len(), 0);
        assert_eq!(gate.deadline(), Some(t + HELD_AT_MOST));
        // An event, and another request's answer, let nothing go.
        assert!(gate.from_server(&from_server(r#"{"type":"event","seq":3,"payload":{}}"#), t).is_empty());
        assert!(gate.from_server(&from_server(&answer(9)), t).is_empty());
        // The first's answer: the second goes, with the resize behind it; the third write waits for the second's.
        let gone = gate.from_server(&from_server(&answer(1)), t);
        assert_eq!(gone, vec![frame(&write(2, "b"), Some([1, 2, 3, 4])), frame(resize, Some([1, 2, 3, 4]))]);
        assert_eq!(gate.from_server(&from_server(&answer(2)), t), vec![frame(&write(4, "c"), Some([1, 2, 3, 4]))]);
        assert!(gate.from_server(&from_server(&answer(4)), t).is_empty());
        assert_eq!(gate.deadline(), None);
        // What isn't typed never waits on anything else.
        assert_eq!(gate.from_app(piece(resize), t).len(), 1);
        assert_eq!(gate.from_app(Piece::Raw(vec![1, 2, 3]), t).len(), 1);
    }

    #[test]
    fn an_answer_that_never_comes_holds_nothing_for_good() {
        let t = Instant::now();
        let mut gate = Gate::new();
        let piece = |text: &str| Piece::Frame { bytes: frame(text, Some([1, 2, 3, 4])), text: Some(text.as_bytes().to_vec()) };
        gate.from_app(piece(&write(1, "a")), t);
        gate.from_app(piece(&write(2, "b")), t + Duration::from_millis(10));
        gate.from_app(piece(&write(3, "c")), t + Duration::from_millis(20));
        assert!(gate.tick(t + HELD_AT_MOST - Duration::from_millis(1)).is_empty());
        // Everything held goes on together: nothing typed is kept
        // longer than that.
        assert_eq!(gate.tick(t + HELD_AT_MOST), vec![frame(&write(2, "b"), Some([1, 2, 3, 4])), frame(&write(3, "c"), Some([1, 2, 3, 4]))]);
        // (And the last of it is waited for in its turn.)
        assert_eq!(gate.from_app(piece(&write(4, "d")), t + HELD_AT_MOST).len(), 0);
        assert!(gate.from_server(&Piece::Frame { bytes: Vec::new(), text: Some(answer(2).into_bytes()) }, t + HELD_AT_MOST).is_empty());
        assert_eq!(gate.from_server(&Piece::Frame { bytes: Vec::new(), text: Some(answer(3).into_bytes()) }, t + HELD_AT_MOST).len(), 1);
    }

    #[test]
    fn a_connection_that_wasnt_upgraded_holds_nothing() {
        let t = Instant::now();
        let mut gate = Gate::new();
        let piece = |text: &str| Piece::Frame { bytes: frame(text, Some([1, 2, 3, 4])), text: Some(text.as_bytes().to_vec()) };
        assert_eq!(gate.from_app(piece(&write(1, "a")), t).len(), 1);
        assert_eq!(gate.from_app(piece(&write(2, "b")), t).len(), 0);
        assert_eq!(gate.open(t).len(), 1);
        assert_eq!(gate.from_app(piece(&write(3, "c")), t).len(), 1);
        assert_eq!(gate.deadline(), None);
    }

    /// What says what a message is, is read whatever else is in it: an
    /// id as text or as a number, the word "response" inside an event,
    /// a method named inside another request's text.
    #[test]
    fn a_message_is_known_by_its_own_type_and_id() {
        assert_eq!(typed(write(7, "x").as_bytes()).as_deref(), Some("7"));
        assert_eq!(typed(br#"{"type":"request","id":12,"method":"client.key","payload":{}}"#).as_deref(), Some("12"));
        assert_eq!(typed(br#"{"type":"request","id":"3","method":"session.view","payload":{"note":"client.key"}}"#), None);
        assert_eq!(typed(br#"{"type":"response","id":"3","payload":{"method":"client.key"}}"#), None);
        assert_eq!(typed(b"client.key, but not a message"), None);
        assert!(answers(answer(7).as_bytes(), "7"));
        assert!(answers(br#"{"type":"response","id":7,"error":{"code":"no"}}"#, "7"));
        assert!(!answers(answer(17).as_bytes(), "7"));
        assert!(!answers(br#"{"type":"event","seq":7,"payload":{"type":"response","id":"7"}}"#, "7"));
        // (Text with an escape in it is read too.)
        assert_eq!(typed(br#"{"type":"request","id":"a\"b","method":"client.key"}"#).as_deref(), Some("a\"b"));
    }

    /// Requests and answers in any order and at any pace: all that the
    /// app sent goes on to the server, once, in its order, and never a
    /// typed request while another is unanswered (short of the time
    /// that lets one go).
    #[test]
    fn anything_sent_at_any_pace_goes_on_whole_and_in_order() {
        let mut rng = Lcg(11);
        for round in 0..300 {
            let t = Instant::now();
            let mut gate = Gate::new();
            let (mut sent, mut passed): (Vec<Vec<u8>>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
            // The typed requests the server has and hasn't answered, as
            // it would see them.
            let mut at_server: VecDeque<u32> = VecDeque::new();
            let mut next_id = 1u32;
            let take = |passed: &mut Vec<Vec<u8>>, at_server: &mut VecDeque<u32>, gone: Vec<Vec<u8>>| {
                for bytes in gone {
                    let mut scanner = Scanner { past_head: true, ..Scanner::new() };
                    if let Some(Piece::Frame { text: Some(text), .. }) = scanner.push(&bytes).pop()
                        && let Some(id) = typed(&text)
                    {
                        assert!(at_server.is_empty(), "round {round}: a typed request reached the server while {at_server:?} was unanswered");
                        at_server.push_back(id.parse().unwrap());
                    }
                    passed.push(bytes);
                }
            };
            for _ in 0..60 {
                match rng.next(5) {
                    0..=2 => {
                        let text = if rng.next(3) == 0 { r#"{"type":"request","id":"x","method":"session.list"}"#.to_owned() } else { write(next_id, "k") };
                        next_id += 1;
                        let bytes = frame(&text, Some([rng.next(256) as u8, 1, 2, 3]));
                        sent.push(bytes.clone());
                        let gone = gate.from_app(Piece::Frame { bytes, text: Some(text.into_bytes()) }, t);
                        take(&mut passed, &mut at_server, gone);
                    }
                    _ => {
                        if let Some(id) = at_server.pop_front() {
                            let text = answer(id);
                            let gone = gate.from_server(&Piece::Frame { bytes: frame(&text, None), text: Some(text.into_bytes()) }, t);
                            take(&mut passed, &mut at_server, gone);
                        }
                    }
                }
            }
            // The server answers what it has, until nothing is held.
            while let Some(id) = at_server.pop_front() {
                let text = answer(id);
                let gone = gate.from_server(&Piece::Frame { bytes: frame(&text, None), text: Some(text.into_bytes()) }, t);
                take(&mut passed, &mut at_server, gone);
            }
            assert_eq!(passed, sent, "round {round}");
        }
    }
}
