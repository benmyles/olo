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
//! answer to the upgrade ([`HERE`]). What it doesn't understand passes
//! untouched.
//!
//! And it says one thing of its own, unasked ([`Teller`]): how a
//! terminal reads, a moment after the server has taken an Enter typed
//! into it and a moment after the program in its foreground changes.
//! The app draws the first letter typed at a shell's prompt at once
//! where the host says the shell itself is reading there; asked from
//! the app that costs a round trip of the link after every command,
//! and here, beside the server, nothing. It asks the server itself
//! (two requests of its own, whose answers go no further) and tells
//! the app in an event the app already knows by name.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub mod agents;

/// What this helper does, counted: the app reads it from [`HEADER`] and
/// asks of the helper only what its number says it does. 1: what is
/// typed is kept in order. 2: and how a terminal reads is said
/// unasked ([`Teller`]).
pub const VERSION: u32 = 2;

/// The name of the header the helper adds to the server's answer to
/// the upgrade, by which the app knows its control connection goes
/// through it, and what the helper does ([`VERSION`]).
pub const HEADER: &str = "X-Olo-Rex-Link";

/// That header, whole.
pub const HERE: &str = "X-Olo-Rex-Link: 2";

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
    /// A message sent in several frames has begun and not ended.
    in_parts: bool,
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

    /// Whether part of a frame too large to keep has passed and the
    /// rest hasn't, or a message sent in several frames has begun and
    /// not ended: nothing can be put into the direction here.
    pub fn mid_frame(&self) -> bool {
        self.passing > 0 || self.in_parts
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
            // (A message in several frames: begun by a frame of text
            // or bytes that isn't its last, ended by a continuation
            // that is. What may come between them says nothing of it.)
            match (first & 0x0f, first & 0x80 != 0) {
                (1 | 2, last) => self.in_parts = !last,
                (0, true) => self.in_parts = false,
                _ => {}
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
    if !WORDS.typed.iter().any(|method| has(text, method)) {
        return None;
    }
    let top: Top = serde_json::from_slice(text).ok()?;
    (top.kind.as_deref() == Some("request") && top.method.as_deref().is_some_and(|m| TYPED.contains(&m))).then(|| top.id.map(|id| id.said())).flatten()
}

/// The id of the request a piece from the app is, if it is one that
/// types into a terminal. (A message is read once for it, here: the
/// gate and the teller are both given what it found, [`Gate::
/// from_app_as`], [`Teller::from_app_as`]. Reading it is most of what
/// a letter costs the helper.)
pub fn typed_in(piece: &Piece) -> Option<String> {
    match piece {
        Piece::Frame { text: Some(text), .. } => typed(text),
        _ => None,
    }
}

/// The id of the request a piece from the server answers, if it is an
/// answer (read once too: [`Gate::answered`], [`Teller::
/// from_server_as`]).
pub fn answered_in(piece: &Piece) -> Option<String> {
    let Piece::Frame { text: Some(text), .. } = piece else { return None };
    // (Looked at only if it holds the word: events don't.)
    if !has(text, &WORDS.response) {
        return None;
    }
    let top: Top = serde_json::from_slice(text).ok()?;
    (top.kind.as_deref() == Some("response")).then(|| top.id.map(|id| id.said())).flatten()
}

/// The words a message is searched for before it is read, each with
/// its searcher made once (making one costs more than a search of a
/// message this short: measured, 2026-11-23, a letter through the
/// helper was 5,000 instructions more with one made at each search).
struct Words {
    typed: [memchr::memmem::Finder<'static>; 2],
    response: memchr::memmem::Finder<'static>,
    process_changed: memchr::memmem::Finder<'static>,
}

static WORDS: std::sync::LazyLock<Words> = std::sync::LazyLock::new(|| {
    let word = |word: &'static str| memchr::memmem::Finder::new(word);
    Words { typed: TYPED.map(word), response: word("\"response\""), process_changed: word("process_changed") }
});

fn has(text: &[u8], word: &memchr::memmem::Finder) -> bool {
    word.find(text).is_some()
}

/// What goes on to the server, and when: everything in the order it
/// came, a request that types into a terminal only once the one before
/// it is answered.
pub struct Gate {
    /// The typed request the server hasn't answered: its id, and since
    /// when.
    waiting: Option<(String, Instant)>,
    /// What came from the app behind it; with each piece, the id of
    /// the typed request it is, if it is one.
    held: VecDeque<(Piece, Option<String>)>,
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
        let typed = typed_in(&piece);
        self.from_app_as(piece, typed, now)
    }

    /// [`Self::from_app`], the piece read already: `typed`,
    /// [`typed_in`] of it.
    pub fn from_app_as(&mut self, piece: Piece, typed: Option<String>, now: Instant) -> Vec<Vec<u8>> {
        self.held.push_back((piece, typed));
        self.pass(now)
    }

    /// A piece from the server (it goes on to the app whatever it is):
    /// what an answer in it lets go on to the server.
    pub fn from_server(&mut self, piece: &Piece, now: Instant) -> Vec<Vec<u8>> {
        if self.waiting.is_none() {
            return Vec::new();
        }
        self.answered(answered_in(piece).as_deref(), now)
    }

    /// [`Self::from_server`], the piece read already: the id of the
    /// request it answers ([`answered_in`]), if it is an answer.
    pub fn answered(&mut self, id: Option<&str>, now: Instant) -> Vec<Vec<u8>> {
        if id.is_some() && self.waiting.as_ref().map(|(waiting, _)| waiting.as_str()) == id {
            self.waiting = None;
            return self.pass(now);
        }
        Vec::new()
    }

    /// Whether a typed request is at the server unanswered (an answer
    /// is of interest).
    pub fn waits(&self) -> bool {
        self.waiting.is_some()
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
        while let Some((piece, typed)) = self.held.pop_front() {
            if let Some(id) = typed {
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

    /// Whether part of a frame too large to keep is among what is
    /// held: its other parts may have gone on already, and nothing can
    /// be put between them.
    pub fn holds_raw(&self) -> bool {
        self.held.iter().any(|(piece, _)| matches!(piece, Piece::Raw(_)))
    }

    fn pass(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some((_, typed)) = self.held.front() {
            let typed = typed.is_some() && !self.open;
            if typed && self.waiting.is_some() {
                break;
            }
            let (piece, id) = self.held.pop_front().unwrap();
            if typed {
                self.waiting = id.map(|id| (id, now));
            }
            out.push(piece.bytes().to_vec());
        }
        out
    }
}

/// How long after the server has taken an Enter the terminal it went to
/// is asked how it reads: a command that does nothing to speak of is
/// over by then and its shell at its prompt again. (And a host so
/// loaded that its shell hasn't run in that time is still asked then,
/// and says what stood before the Enter: the app trusts the word only
/// with a prompt it knows drawn again after it, HACKING, "A shell's
/// prompt said unasked".)
pub const AFTER_ENTER: Duration = Duration::from_millis(80);

/// How long after the server says the program in a terminal's
/// foreground changed the terminal is asked: a shell back in the
/// foreground sets the terminal up for its line editor a moment after.
/// And the first of the waits before a terminal that didn't read is
/// asked again ([`ASKED_AGAIN`]), each twice the one before.
pub const AFTER_PROCESS: Duration = Duration::from_millis(40);

/// How many more times a terminal is asked while its shell isn't
/// reading there: 40, 80, 160, 320 and 640 ms on, which is a command
/// over within a second and a quarter, a prompt that takes its time
/// to be drawn (one that looks at a repository first), and a command
/// too quick for the server to have said it ran. Then no more, until
/// the server says the foreground changed or another line is ended:
/// nothing is asked of a terminal where nothing happens.
pub const ASKED_AGAIN: u8 = 5;

/// The requests the helper makes of the server itself, and the mark of
/// their ids (no id of the app's begins so: its are numbers).
const INPUT_MODE: &str = "com.superlogical.terminal.input_mode";
const PROCESS: &str = "com.superlogical.terminal.process";
const OURS: &str = "olo-rex-link-";

/// The most of anything the teller keeps: past it the oldest is let go
/// (a server that never answers isn't kept count of for ever).
const MOST_KEPT: usize = 32;

/// A terminal, as a request names it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Terminal {
    session: String,
    block: String,
}

/// A terminal to be asked about, when, and how many times it has been
/// since something happened there.
struct Due {
    at: Instant,
    of: Terminal,
    tries: u8,
}

/// A question of the helper's own on its way: about which terminal,
/// and what has come back of its two halves (the mode: echo,
/// canonical, signals; and whether the foreground process is the
/// terminal's own first, its shell).
struct Asked {
    n: u64,
    of: Terminal,
    mode: Option<(bool, bool, bool)>,
    shell: Option<bool>,
    /// The last thing typed that the server had taken when this was
    /// asked.
    after: Option<String>,
    /// Something was typed since it was asked: what it says may be of
    /// before that or of after, and isn't said.
    stale: bool,
    tries: u8,
}

/// What was last said of a terminal (said again only when it changes).
#[derive(PartialEq)]
struct Said {
    mode: (bool, bool, bool),
    shell: bool,
    /// For a terminal that reads: after what typed.
    after: Option<String>,
}

#[derive(serde::Deserialize)]
struct Write<'a> {
    #[serde(borrow)]
    payload: Option<WriteTo<'a>>,
}

#[derive(serde::Deserialize)]
struct WriteTo<'a> {
    #[serde(borrow)]
    session_id: Option<std::borrow::Cow<'a, str>>,
    #[serde(borrow)]
    block_id: Option<std::borrow::Cow<'a, str>>,
    #[serde(borrow)]
    args: Option<WriteArgs<'a>>,
}

#[derive(serde::Deserialize)]
struct WriteArgs<'a> {
    #[serde(borrow)]
    data: Option<std::borrow::Cow<'a, str>>,
}

/// The terminal a write ends a line in, if it is one that does.
fn line_ended(text: &[u8]) -> Option<Terminal> {
    // (Looked at only if it is a write with a line's end in it, which
    // in a request is the escape `\r` or `\n`: most have none.)
    if !memchr::memchr_iter(b'\\', text).any(|at| matches!(text.get(at + 1), Some(b'r' | b'n'))) {
        return None;
    }
    let to = serde_json::from_slice::<Write>(text).ok()?.payload?;
    let (session, block) = (to.session_id?, to.block_id?);
    to.args?.data?.contains(['\r', '\n']).then(|| Terminal { session: session.into_owned(), block: block.into_owned() })
}

/// What the helper says of a terminal unasked: how it reads (its input
/// mode, and whether its shell is in the foreground), a moment after
/// the server took an Enter typed into it ([`AFTER_ENTER`]) and a
/// moment after the server said its foreground process changed
/// ([`AFTER_PROCESS`]); and a few times more while the shell isn't
/// reading there ([`ASKED_AGAIN`]). It asks the server with two
/// requests of its own and gives the app one event of the terminal's,
/// `input_mode_changed`, with `echo`, `canonical`, `signals`, `shell`
/// and `after`; only when it says something other than the last did.
///
/// **What the word is good for.** `after` is the id of the last
/// request typed (on this connection, into any terminal) that the
/// server had answered when the helper asked, and the word is only
/// given if nothing typed was unanswered then and nothing typed came
/// before both answers were in. So an app whose own last typed request
/// is `after` knows the terminal was looked at after everything it
/// typed had been taken, and one that has typed since knows the word
/// is of before that.
///
/// Everything else passes: [`Teller::from_server`] takes out only the
/// answers to its own requests.
#[derive(Default)]
pub struct Teller {
    /// What the app typed that the server hasn't answered: each
    /// request's id, and the terminal it ends a line in, if it does.
    typed: VecDeque<(String, Option<Terminal>)>,
    /// The last of them the server answered.
    last: Option<String>,
    due: Vec<Due>,
    asked: Vec<Asked>,
    /// How many answers to the helper's own requests are still to
    /// come (the server's messages are searched for them only then).
    owed: u32,
    said: Vec<(Terminal, Said)>,
    next: u64,
    off: bool,
}

impl Teller {
    pub fn new() -> Teller {
        Teller::default()
    }

    /// The server's side isn't a connection to ask on (the upgrade was
    /// refused): nothing is asked or said from here.
    pub fn off(&mut self) {
        self.off = true;
        self.typed.clear();
        self.due.clear();
    }

    /// A piece from the app, as it passes: what is typed is noted (a
    /// question on its way is of before it or of after, and says
    /// nothing now), and where it ends a line, its terminal is asked
    /// about once the server has taken it.
    pub fn from_app(&mut self, piece: &Piece) {
        self.from_app_as(piece, typed_in(piece).as_deref());
    }

    /// [`Self::from_app`], the piece read already: `typed`,
    /// [`typed_in`] of it.
    pub fn from_app_as(&mut self, piece: &Piece, typed: Option<&str>) {
        let (Piece::Frame { text: Some(text), .. }, Some(id), false) = (piece, typed, self.off) else { return };
        for asked in &mut self.asked {
            asked.stale = true;
        }
        if self.typed.len() >= MOST_KEPT {
            self.typed.pop_front();
        }
        self.typed.push_back((id.to_owned(), line_ended(text)));
    }

    /// Whether an answer of the server's is of interest: to something
    /// typed, or to a request of the helper's own.
    pub fn waits(&self) -> bool {
        !self.typed.is_empty() || self.owed > 0
    }

    /// A piece from the server. What goes to the app in its place, if
    /// it isn't to pass as it is: nothing, for an answer to one of the
    /// helper's own requests, or the event that says how the terminal
    /// reads, once both answers are in.
    pub fn from_server(&mut self, piece: &Piece, now: Instant) -> Option<Vec<u8>> {
        let answered = if self.waits() { answered_in(piece) } else { None };
        self.from_server_as(piece, answered.as_deref(), now)
    }

    /// [`Self::from_server`], the piece read already: the id of the
    /// request it answers ([`answered_in`]), if it is an answer and
    /// one is of interest ([`Self::waits`]).
    pub fn from_server_as(&mut self, piece: &Piece, answered: Option<&str>, now: Instant) -> Option<Vec<u8>> {
        let Piece::Frame { text: Some(text), .. } = piece else { return None };
        if self.off {
            return None;
        }
        if let Some(id) = answered {
            if self.owed > 0
                && id.starts_with(OURS)
                && let Some(instead) = self.answered(text, now)
            {
                self.owed -= 1;
                return Some(instead);
            }
            // (The answer to something typed, and so to all typed
            // before it: the terminals they ended lines in are asked
            // about in a moment.)
            if let Some(at) = self.typed.iter().position(|(typed, _)| typed == id) {
                // (Several only where an answer never came: seldom.)
                for _ in 0..=at {
                    let Some((typed, of)) = self.typed.pop_front() else { break };
                    self.last = Some(typed);
                    if let Some(of) = of {
                        self.ask(of, now + AFTER_ENTER, 0);
                    }
                }
            }
        } else if has(text, &WORDS.process_changed)
            && let Ok(event) = serde_json::from_slice::<serde_json::Value>(text)
            && let Some(block) = event.pointer("/payload/payload/event/payload")
            // (The server names a block's event `name`; `event` is read too, as the app reads either.)
            && block.get("name").or_else(|| block.get("event")).and_then(|e| e.as_str()) == Some("process_changed")
            && let (Some(session), Some(block)) =
                (event.pointer("/payload/payload/session_id").and_then(|s| s.as_str()), block.get("block_id").and_then(|b| b.as_str()))
        {
            self.ask(Terminal { session: session.to_owned(), block: block.to_owned() }, now + AFTER_PROCESS, 0);
        }
        None
    }

    /// One question waits for a terminal at a time: of two, the one
    /// for what happened last (the fewer times asked since), and of
    /// those the later.
    fn ask(&mut self, of: Terminal, at: Instant, tries: u8) {
        if let Some(due) = self.due.iter_mut().find(|due| due.of == of) {
            if tries <= due.tries {
                (due.at, due.tries) = (if tries == due.tries { at.max(due.at) } else { at }, tries);
            }
            return;
        }
        if self.due.len() >= MOST_KEPT {
            self.due.remove(0);
        }
        self.due.push(Due { at, of, tries });
    }

    /// When [`Self::tick`] next has something to ask: never while
    /// something typed is unanswered (it asks once that is taken).
    pub fn deadline(&self) -> Option<Instant> {
        self.due.iter().map(|due| due.at).min().filter(|_| self.typed.is_empty())
    }

    /// What is to be asked of the server now: the requests, each a
    /// frame as a client sends it.
    pub fn tick(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if !self.typed.is_empty() {
            return out;
        }
        let mut i = 0;
        while i < self.due.len() {
            if self.due[i].at > now {
                i += 1;
                continue;
            }
            let Due { of, tries, .. } = self.due.remove(i);
            self.next += 1;
            let n = self.next;
            for (method, half) in [(INPUT_MODE, 'm'), (PROCESS, 'p')] {
                let request = serde_json::json!({ "type": "request", "id": format!("{OURS}{n}-{half}"), "method": method, "payload": { "session_id": of.session, "block_id": of.block } });
                out.push(client_frame(request.to_string().as_bytes()));
                self.owed += 1;
            }
            if self.asked.len() >= MOST_KEPT {
                self.asked.remove(0);
            }
            self.asked.push(Asked { n, of, mode: None, shell: None, after: self.last.clone(), stale: false, tries });
        }
        out
    }

    /// An answer to one of the helper's own requests: it goes no
    /// further; with the second of a pair, the event that says both,
    /// if it says something new. `None` for what isn't one.
    fn answered(&mut self, text: &[u8], now: Instant) -> Option<Vec<u8>> {
        let answer: serde_json::Value = serde_json::from_slice(text).ok()?;
        let id = answer.get("id")?.as_str()?.strip_prefix(OURS)?;
        if answer.get("type").and_then(|t| t.as_str()) != Some("response") {
            return None;
        }
        // (One of ours, whatever it says: not the app's to see. One
        // whose question is no longer kept says nothing.)
        let nothing = Some(Vec::new());
        let Some((n, half)) = id.split_once('-').and_then(|(n, half)| Some((n.parse::<u64>().ok()?, half))) else { return nothing };
        let Some(at) = self.asked.iter().position(|asked| asked.n == n) else { return nothing };
        let asked = &mut self.asked[at];
        let payload = answer.get("payload").filter(|_| answer.get("error").is_none_or(|e| e.is_null()));
        let flag = |name: &str| payload.and_then(|p| p.get(name)).and_then(|v| v.as_bool());
        let failed = match half {
            "m" => {
                asked.mode = flag("echo").zip(flag("canonical")).map(|(echo, canonical)| (echo, canonical, flag("signals").unwrap_or(true)));
                asked.mode.is_none()
            }
            _ => {
                let pid = |of: &str| payload.and_then(|p| p.pointer(&format!("/{of}/pid"))).and_then(|pid| pid.as_u64());
                asked.shell = payload.map(|_| matches!((pid("child"), pid("foreground")), (Some(child), Some(foreground)) if child == foreground && child != 0));
                asked.shell.is_none()
            }
        };
        if failed {
            self.asked.remove(at);
            return nothing;
        }
        let (Some(mode), Some(shell)) = (asked.mode, asked.shell) else { return nothing };
        let asked = self.asked.remove(at);
        if asked.stale {
            return nothing;
        }
        let (echo, canonical, signals) = mode;
        let reads = shell && !echo && !canonical;
        if !reads && asked.tries < ASKED_AGAIN {
            self.ask(asked.of.clone(), now + AFTER_PROCESS * (1 << asked.tries), asked.tries + 1);
        }
        let said = Said { mode, shell, after: asked.after.clone().filter(|_| reads) };
        if self.said.iter().any(|(of, last)| *of == asked.of && *last == said) {
            return nothing;
        }
        self.said.retain(|(of, _)| *of != asked.of);
        if self.said.len() >= MOST_KEPT {
            self.said.remove(0);
        }
        self.said.push((asked.of.clone(), said));
        // (Numbered 0, as nothing of the server's is: it has no place
        // among the server's own events, and takes none.)
        let event = serde_json::json!({
            "type": "event",
            "seq": 0,
            "payload": { "type": "session_event", "payload": { "session_id": asked.of.session, "event": { "type": "block_event", "payload": {
                "block_id": asked.of.block,
                "creator_name": "com.superlogical.terminal",
                "name": "input_mode_changed",
                "payload": { "echo": echo, "canonical": canonical, "signals": signals, "shell": shell, "after": asked.after },
            } } } },
        });
        Some(server_frame(event.to_string().as_bytes()))
    }
}

/// A text message in one frame, as a server sends it.
fn server_frame(text: &[u8]) -> Vec<u8> {
    let mut out = vec![0x81];
    length(&mut out, text.len(), 0);
    out.extend_from_slice(text);
    out
}

/// A text message in one frame, as a client sends it: masked, as a
/// client's must be (with a key of nothing, which leaves it as it is:
/// the masking is against proxies between a browser and a server, and
/// there is none between the helper and the server beside it).
fn client_frame(text: &[u8]) -> Vec<u8> {
    let mut out = vec![0x81];
    length(&mut out, text.len(), 0x80);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(text);
    out
}

fn length(out: &mut Vec<u8>, n: usize, masked: u8) {
    match n {
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

    /// A piece as a scanner gives it: a text frame and what it says.
    fn said(text: &str, mask: Option<[u8; 4]>) -> Piece {
        Piece::Frame { bytes: frame(text, mask), text: Some(text.as_bytes().to_vec()) }
    }

    fn text_of(frame: &[u8]) -> serde_json::Value {
        let mut scanner = Scanner::new();
        scanner.past_head = true;
        match scanner.push(frame).pop() {
            Some(Piece::Frame { text: Some(text), .. }) => serde_json::from_slice(&text).unwrap(),
            other => panic!("not a text frame: {other:?}"),
        }
    }

    /// Nothing of the helper's own can be put into a direction while a
    /// frame too large to keep is passing, or between the frames of a
    /// message sent in several.
    #[test]
    fn a_message_in_parts_and_a_frame_passing_are_not_between_frames() {
        let mut scanner = Scanner::new();
        scanner.past_head = true;
        scanner.push(&frame("whole", Some([1, 2, 3, 4])));
        assert!(!scanner.mid_frame());
        // A text message in three frames, a ping between two of them.
        let mut first = frame("be", Some([1, 2, 3, 4]));
        first[0] = 0x01;
        scanner.push(&first);
        assert!(scanner.mid_frame(), "begun, not ended");
        let mut ping = frame("", Some([0; 4]));
        ping[0] = 0x89;
        scanner.push(&ping);
        let mut middle = frame("gu", Some([1, 2, 3, 4]));
        middle[0] = 0x00;
        scanner.push(&middle);
        assert!(scanner.mid_frame());
        let mut last = frame("n", Some([1, 2, 3, 4]));
        last[0] = 0x80;
        scanner.push(&last);
        assert!(!scanner.mid_frame(), "ended");
        // A frame too large to keep: mid-frame from its header to its last byte.
        let mut large = vec![0x82, 127];
        large.extend_from_slice(&((LARGEST as u64 + 1).to_be_bytes()));
        scanner.push(&large);
        scanner.push(&vec![0; LARGEST]);
        assert!(scanner.mid_frame());
        scanner.push(&[0]);
        assert!(!scanner.mid_frame());
    }

    /// The helper's two questions about a terminal, as the teller gave
    /// them: checked, and their ids.
    fn questions(asked: &[Vec<u8>]) -> (String, String) {
        let asked: Vec<serde_json::Value> = asked.iter().map(|frame| text_of(frame)).collect();
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert_eq!(asked[0]["method"], "com.superlogical.terminal.input_mode");
        assert_eq!(asked[1]["method"], "com.superlogical.terminal.process");
        for request in &asked {
            assert_eq!((request["type"].as_str(), request["payload"]["session_id"].as_str(), request["payload"]["block_id"].as_str()), (Some("request"), Some("s"), Some("b")));
        }
        (asked[0]["id"].as_str().unwrap().to_owned(), asked[1]["id"].as_str().unwrap().to_owned())
    }

    fn mode(id: &str, echo: bool, canonical: bool) -> Piece {
        said(&format!(r#"{{"type":"response","id":"{id}","seq":41,"payload":{{"echo":{echo},"canonical":{canonical},"signals":true}}}}"#), None)
    }

    fn process(id: &str, foreground: u32) -> Piece {
        said(&format!(r#"{{"type":"response","id":"{id}","seq":42,"payload":{{"child":{{"pid":24,"name":"bash"}},"foreground":{{"pid":{foreground},"name":"x"}}}}}}"#), None)
    }

    /// What an event of the teller's says of the terminal.
    fn word(event: Option<Vec<u8>>) -> serde_json::Value {
        let event = text_of(&event.expect("an event in the answer's place"));
        assert_eq!((event["type"].as_str(), event["seq"].as_u64(), event["payload"]["type"].as_str()), (Some("event"), Some(0), Some("session_event")), "numbered 0: none of the server's own");
        assert_eq!(event["payload"]["payload"]["session_id"], "s");
        let block = &event["payload"]["payload"]["event"];
        assert_eq!((block["type"].as_str(), block["payload"]["block_id"].as_str(), block["payload"]["name"].as_str()), (Some("block_event"), Some("b"), Some("input_mode_changed")));
        block["payload"]["payload"].clone()
    }

    /// (As the server writes it: the event's name under `name`. Read
    /// from a real one, 2026-11-23; the helper had looked for `event`,
    /// which the app reads too, and never saw a command end.)
    const CHANGED: &str = r#"{"type":"event","seq":50,"payload":{"type":"session_event","payload":{"session_id":"s","event":{"type":"block_event","payload":{"block_id":"b","creator_name":"com.superlogical.terminal","name":"process_changed","payload":{"generation":4,"foreground":{"pid":30,"user_id":1000,"name":"sudo","argv0":"sudo"}}}}}}}"#;

    /// How a terminal reads is asked of the server a moment after it
    /// took an Enter typed into that terminal, and a moment after it
    /// said the terminal's foreground process changed; the two answers
    /// go no further, and the app is given one event that says both,
    /// and after what typed.
    #[test]
    fn how_a_terminal_reads_is_asked_after_an_enter_and_said_in_one_event() {
        let t = Instant::now();
        let mut teller = Teller::new();
        // A write with no Enter in it, and its answer: nothing to ask.
        teller.from_app(&said(&write(1, "ls"), Some([1, 2, 3, 4])));
        assert_eq!(teller.from_server(&said(&answer(1), None), t), None);
        assert_eq!(teller.deadline(), None);
        // One with an Enter: asked about a moment after its answer, not before.
        teller.from_app(&said(&write(2, "ls\\r"), Some([9, 9, 9, 9])));
        assert_eq!(teller.deadline(), None, "the server hasn't taken it yet");
        assert_eq!(teller.from_server(&said(&answer(2), None), t), None, "the answer passes");
        assert_eq!(teller.deadline(), Some(t + AFTER_ENTER));
        assert!(teller.tick(t + AFTER_ENTER - Duration::from_millis(1)).is_empty());
        let (m, p) = questions(&teller.tick(t + AFTER_ENTER));
        assert_eq!(teller.deadline(), None);
        // The answers: neither goes on; with the second, the event.
        assert_eq!(teller.from_server(&mode(&m, false, false), t), Some(Vec::new()));
        let reads = word(teller.from_server(&process(&p, 24), t));
        assert_eq!(reads, serde_json::json!({ "echo": false, "canonical": false, "signals": true, "shell": true, "after": "2" }));
        assert_eq!(teller.deadline(), None, "a shell that reads isn't asked again");

        // The server's word that a foreground process changed: asked
        // again, sooner; another program in the foreground isn't the
        // shell, whichever answer comes last.
        assert_eq!(teller.from_server(&said(CHANGED, None), t), None, "the server's own events pass");
        assert_eq!(teller.deadline(), Some(t + AFTER_PROCESS));
        // (Named either way, as the app reads it.)
        let mut other = Teller::new();
        other.from_server(&said(&CHANGED.replace(r#""name":"process_changed""#, r#""event":"process_changed""#), None), t);
        assert_eq!(other.deadline(), Some(t + AFTER_PROCESS));
        let (m, p) = questions(&teller.tick(t + AFTER_PROCESS));
        assert_eq!(teller.from_server(&process(&p, 30), t), Some(Vec::new()));
        let asks = word(teller.from_server(&mode(&m, false, true), t));
        assert_eq!(asks, serde_json::json!({ "echo": false, "canonical": true, "signals": true, "shell": false, "after": "2" }));
    }

    /// A terminal whose shell isn't reading is asked again, a few
    /// times and each wait twice the last, and then no more; what it
    /// says is given to the app only when it changes; and the shell
    /// back at its prompt is said at the first look that finds it.
    #[test]
    fn a_terminal_that_does_not_read_is_asked_a_few_times_more_and_only_what_changes_is_said() {
        let t = Instant::now();
        let mut teller = Teller::new();
        teller.from_app(&said(&write(1, "make\\r"), None));
        teller.from_server(&said(&answer(1), None), t);
        let mut at = t + AFTER_ENTER;
        let (m, p) = questions(&teller.tick(at));
        teller.from_server(&mode(&m, true, true), at);
        let first = word(teller.from_server(&process(&p, 77), at));
        assert_eq!(first, serde_json::json!({ "echo": true, "canonical": true, "signals": true, "shell": false, "after": "1" }));
        // Four more looks at a command still running: asked, nothing said.
        for again in 0..4u32 {
            assert_eq!(teller.deadline(), Some(at + AFTER_PROCESS * (1 << again)), "look {again}");
            at += AFTER_PROCESS * (1 << again);
            let (m, p) = questions(&teller.tick(at));
            assert_eq!(teller.from_server(&mode(&m, true, true), at), Some(Vec::new()));
            assert_eq!(teller.from_server(&process(&p, 77), at), Some(Vec::new()), "the same as was said: not said again");
        }
        // The fifth finds the shell back, its prompt not drawn yet (it
        // reads nothing, the terminal as a command leaves it): said,
        // and that was the last look of its own.
        at += AFTER_PROCESS * 16;
        let (m, p) = questions(&teller.tick(at));
        teller.from_server(&mode(&m, true, true), at);
        assert_eq!(word(teller.from_server(&process(&p, 24), at))["shell"], true);
        assert_eq!(teller.deadline(), None, "asked {ASKED_AGAIN} times more, and no more");
        // The server's word starts it again; the prompt is there now.
        teller.from_server(&said(CHANGED, None), at);
        let (m, p) = questions(&teller.tick(at + AFTER_PROCESS));
        teller.from_server(&mode(&m, false, false), at);
        assert_eq!(word(teller.from_server(&process(&p, 24), at)), serde_json::json!({ "echo": false, "canonical": false, "signals": true, "shell": true, "after": "1" }));
        // The same again, after nothing typed: not said twice.
        teller.from_server(&said(CHANGED, None), at);
        let (m, p) = questions(&teller.tick(at + AFTER_PROCESS));
        teller.from_server(&mode(&m, false, false), at);
        assert_eq!(teller.from_server(&process(&p, 24), at), Some(Vec::new()));
        // After another line, the same mode is news: it is of after that line.
        teller.from_app(&said(&write(2, "\\r"), None));
        teller.from_server(&said(&answer(2), None), at);
        let (m, p) = questions(&teller.tick(at + AFTER_ENTER));
        teller.from_server(&mode(&m, false, false), at);
        assert_eq!(word(teller.from_server(&process(&p, 24), at))["after"], "2");
    }

    /// The word is of after everything typed, or it isn't given: no
    /// question is asked while something typed is unanswered, and one
    /// that something was typed after says nothing.
    #[test]
    fn a_word_that_typing_overtook_is_not_given() {
        let t = Instant::now();
        let mut teller = Teller::new();
        teller.from_app(&said(&write(1, "ls\\r"), None));
        teller.from_server(&said(&answer(1), None), t);
        // A letter typed before the question's time, unanswered at it:
        // not asked yet, and nothing waited for meanwhile.
        teller.from_app(&said(&write(2, "g"), None));
        assert_eq!(teller.deadline(), None);
        assert!(teller.tick(t + AFTER_ENTER).is_empty());
        // Answered: asked now, late as it is, and of after that letter.
        teller.from_server(&said(&answer(2), None), t + AFTER_ENTER * 2);
        assert_eq!(teller.deadline(), Some(t + AFTER_ENTER));
        let (m, p) = questions(&teller.tick(t + AFTER_ENTER * 2));
        // Another letter while the question is on its way: its answers
        // are taken, and say nothing.
        teller.from_app(&said(&write(3, "i"), None));
        assert_eq!(teller.from_server(&mode(&m, false, false), t), Some(Vec::new()));
        assert_eq!(teller.from_server(&process(&p, 24), t), Some(Vec::new()), "of before the letter or of after: not said");
        teller.from_server(&said(&answer(3), None), t);
        assert_eq!(teller.deadline(), None, "and not asked again for a letter");
        // A key the host's own bindings are asked about is typed too.
        teller.from_app(&said(&write(4, "x\\r"), None));
        teller.from_server(&said(&answer(4), None), t);
        teller.from_app(&said(r#"{"type":"request","id":"5","method":"client.key","payload":{"session_id":"s","block_id":"b","key":{"key":"a"}}}"#, None));
        assert!(teller.tick(t + AFTER_ENTER).is_empty());
        teller.from_server(&said(&answer(5), None), t);
        let (m, p) = questions(&teller.tick(t + AFTER_ENTER));
        teller.from_server(&mode(&m, false, false), t);
        assert_eq!(word(teller.from_server(&process(&p, 24), t))["after"], "5");
    }

    /// What isn't the teller's passes as it is, and what goes wrong
    /// says nothing: an answer the server gives as an error, an answer
    /// to a question no longer kept, the app's own requests of the same
    /// methods, and a connection that isn't one to ask on.
    #[test]
    fn the_teller_takes_only_its_own_and_says_nothing_of_what_failed() {
        let t = Instant::now();
        let mut teller = Teller::new();
        // The app's own question and its answer: theirs.
        let own = r#"{"type":"request","id":"7","method":"com.superlogical.terminal.input_mode","payload":{"session_id":"s","block_id":"b"}}"#;
        teller.from_app(&said(own, Some([1, 1, 1, 1])));
        assert_eq!(teller.from_server(&said(r#"{"type":"response","id":"7","seq":3,"payload":{"echo":true,"canonical":true,"signals":true}}"#, None), t), None);
        // An event that only names the helper (a process of that name): the server's.
        let named = r#"{"type":"event","seq":4,"payload":{"type":"session_event","payload":{"session_id":"s","event":{"type":"block_event","payload":{"block_id":"b","event":"title_changed","payload":{"title":"olo-rex-link-1-m \"response\""}}}}}}"#;
        assert_eq!(teller.from_server(&said(named, None), t), None);
        // A failure: the pair is dropped, and its other half says nothing either.
        teller.from_app(&said(&write(8, "x\\n"), Some([2, 2, 2, 2])));
        teller.from_server(&said(&answer(8), None), t);
        let (m, p) = questions(&teller.tick(t + AFTER_ENTER));
        let failed = format!(r#"{{"type":"response","id":"{m}","seq":9,"error":{{"code":"not_found","message":"no such block"}}}}"#);
        assert_eq!(teller.from_server(&said(&failed, None), t), Some(Vec::new()));
        assert_eq!(teller.from_server(&process(&p, 24), t), Some(Vec::new()), "one of ours, though its question is gone");
        assert_eq!(teller.deadline(), None, "and a terminal that can't be asked isn't asked again");
        // Two Enters into one terminal before either is asked about: one question.
        teller.from_app(&said(&write(11, "a\\r"), None));
        teller.from_app(&said(&write(12, "b\\r"), None));
        teller.from_server(&said(&answer(12), None), t);
        assert_eq!(teller.tick(t + AFTER_ENTER).len(), 2, "one pair for the terminal");
        assert!(teller.typed.is_empty(), "the write before it was answered too");
        // Off: nothing is noted, asked or taken.
        let mut teller = Teller::new();
        teller.off();
        teller.from_app(&said(&write(1, "x\\r"), None));
        assert_eq!(teller.from_server(&said(&answer(1), None), t), None);
        assert_eq!(teller.deadline(), None);
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
        assert_eq!(String::from_utf8(said).unwrap(), "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: abc\r\nX-Olo-Rex-Link: 2\r\n\r\n");
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
        let answers = |text: &[u8], id: &str| answered_in(&Piece::Frame { bytes: Vec::new(), text: Some(text.to_vec()) }).as_deref() == Some(id);
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

/// What the app and the helper agree on for the helper's own link over
/// UDP (QUIC; the helper's `quic` mode, the app's `olo_rex_client::
/// quic`).
///
/// The helper is started over SSH (`olo-rex-link quic -- DIAL…`), is
/// given a token on its input (a line of 64 hex digits, made by the
/// app for this link alone), listens on a UDP port with a certificate
/// it makes then, and says on its output one line, [`ANNOUNCE`], its
/// [`VERSION`](quic_wire::VERSION), the port, and the certificate's
/// SHA-256 in hex: so the app knows the certificate by a channel SSH
/// vouches for, and anybody who reaches the port is nobody without the
/// token. Each stream the app opens begins with the token and a kind:
/// [`PLAIN`](quic_wire::PLAIN) is joined to a `DIAL` of its own (a
/// stream to the host's Rex server, as an SSH channel was),
/// [`CONTROL`](quic_wire::CONTROL) to the helper's gate in front of
/// one, [`HELLO`](quic_wire::HELLO) is answered with a byte (the link
/// is up).
pub mod quic_wire {
    pub const ALPN: &[u8] = b"olo-rex-1";
    /// The name in the helper's certificate (nobody looks it up).
    pub const NAME: &str = "olo-rex";
    pub const ANNOUNCE: &str = "olo-rex-quic";
    pub const VERSION: u32 = 1;
    pub const TOKEN: usize = 32;
    pub const PLAIN: u8 = 0;
    pub const CONTROL: u8 = 1;
    pub const HELLO: u8 = 2;
    /// The ports the helper tries first (one to open in a firewall),
    /// before any the system gives it.
    pub const PORTS: std::ops::RangeInclusive<u16> = 61100..=61199;
    /// A link with nothing passing ends after so long (each end), and
    /// the app says it is there this often meanwhile: a path that died
    /// is known within the first, and a NAT keeps its way open.
    pub const IDLE: std::time::Duration = std::time::Duration::from_secs(30);
    pub const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(10);
}
